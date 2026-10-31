use std::cell::RefCell;
use std::ffi::OsString;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use crate::cancel::{self, Cancel};
use crate::keychain::{
    Keychain, KeychainError, LockState, ReadCounts, empty_service_err, empty_service_read,
    locked_err,
};
use crate::process::{self, Captured, DRAIN_GRACE, Output, SpawnSpec};
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
    /// The process did not finish in time: it hung (and its group was killed if it could be), or it
    /// exited but never closed its output pipes within the grace period.
    TimedOut,
    /// The cancel token was set: the child's group was killed, or it was never spawned (H4).
    Interrupted(i32),
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
            RunResult::Interrupted(signal) => f.debug_tuple("Interrupted").field(signal).finish(),
            RunResult::SpawnFailed(e) => f.debug_tuple("SpawnFailed").field(e).finish(),
        }
    }
}

pub trait Runner: Send + Sync {
    /// Runs without the terminal: stdin piped (`stdin` given) or null, stdout and stderr
    /// captured. The child leads a process group of its own (§14.1), so a Ctrl-C at the terminal
    /// never reaches it and never cuts a Keychain write short. After `timeout` its whole group is
    /// killed (`TimedOut`); once `cancel` is set it is killed the same way (`Interrupted`), and a
    /// token already set runs nothing (H4).
    fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        timeout: Duration,
        cancel: Option<&Cancel>,
    ) -> RunResult;
    /// Runs with the terminal attached: stdin and stderr inherited, the child's stdout sent to
    /// stderr (stdout is reserved for command output, §14). No timeout, because a person is
    /// answering. `Exited` carries no output. The child stays in tagteam's process group, the
    /// terminal's foreground one, so a Ctrl-C there reaches it too (§14.1).
    fn run_attached(&self, program: &str, args: &[String]) -> RunResult;
}

pub struct ProcessRunner;

impl ProcessRunner {
    /// `process::bounded` (H4, the one bounded runner) with its ending as a `RunResult`. The
    /// environment is inherited and no directory is set. A signal's end is rc -1.
    fn bounded(
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        timeout: Duration,
        cancel: Option<&Cancel>,
        grace: Duration,
    ) -> RunResult {
        let spec = SpawnSpec {
            program: program.into(),
            args: args.iter().map(OsString::from).collect(),
            ..SpawnSpec::default()
        };
        let stdin = stdin.map(<[u8]>::to_vec);
        match process::bounded(&spec, stdin, Output::Pipe, timeout, cancel, grace) {
            Captured::Exited {
                code,
                stdout,
                stderr,
                ..
            } => RunResult::Exited {
                code: code.unwrap_or(-1),
                stdout,
                stderr,
            },
            Captured::TimedOut => RunResult::TimedOut,
            Captured::Interrupted(signal) => RunResult::Interrupted(signal),
            Captured::SpawnFailed(e) => RunResult::SpawnFailed(e),
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
        cancel: Option<&Cancel>,
    ) -> RunResult {
        Self::bounded(program, args, stdin, timeout, cancel, DRAIN_GRACE)
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
        && bytes.len().is_multiple_of(2)
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
        if digits.is_empty() || !digits.len().is_multiple_of(2) {
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
/// An interruption is the read's, as at the `-w` call (H4).
fn disambiguation_failed(r: &RunResult) -> ReadError {
    let detail = match r {
        RunResult::Exited { code, .. } => format!("rc {code}: the -g disambiguation call failed"),
        RunResult::TimedOut => "the -g disambiguation call did not finish in time".to_owned(),
        RunResult::SpawnFailed(e) => format!("the -g disambiguation call could not run: {e}"),
        RunResult::Interrupted(signal) => return ReadError::interrupted("keychain", *signal),
    };
    ReadError::new("keychain", detail)
}

pub struct SecurityCli {
    runner: Box<dyn Runner>,
    keychain_file: Option<PathBuf>,
    /// D3 change 6: how this driver's secret reads ended (Decision 7).
    counts: Arc<ReadCounts>,
}

impl SecurityCli {
    pub fn new() -> Self {
        Self {
            runner: Box::new(ProcessRunner),
            keychain_file: None,
            counts: ReadCounts::new(),
        }
    }

    /// `keychain_file` targets a specific keychain (tests only); production searches the
    /// default list, as CC does.
    pub fn with_runner(runner: Box<dyn Runner>, keychain_file: Option<PathBuf>) -> Self {
        Self {
            runner,
            keychain_file,
            counts: ReadCounts::new(),
        }
    }

    fn run(&self, mut args: Vec<String>, stdin: Option<&[u8]>) -> RunResult {
        if let Some(f) = &self.keychain_file
            && stdin.is_none()
        {
            args.push(f.to_string_lossy().into_owned());
        }
        // H4: the token of this thread's scope, if any. A write's scope has none.
        let scope = cancel::scoped();
        self.runner
            .run(SECURITY, &args, stdin, TIMEOUT, scope.as_ref())
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
        RunResult::Interrupted(signal) => (None, format!("interrupted by signal {signal}")),
    }
}

/// A failed read: rc 36 is the locked keychain's refusal, and a cancel is an interruption (H4).
fn unreadable(r: RunResult) -> ReadError {
    if let RunResult::Interrupted(signal) = r {
        return ReadError::interrupted("keychain", signal);
    }
    let (rc, message) = describe(&r);
    let detail = match rc {
        Some(code) => format!("rc {code}: {message}"),
        None => message,
    };
    if rc == Some(36) {
        ReadError::locked("keychain", detail)
    } else {
        ReadError::new("keychain", detail)
    }
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

/// A credential as the text of a `security -i` `-w "…"` word (H5.1): `None` unless it is UTF-8
/// with no NUL, CR or LF; `\` is written `\\` and `"` written `\"`, nothing else is touched.
pub(crate) fn escape_text(credential: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(credential).ok()?;
    if text.contains(['\0', '\r', '\n']) {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\\' || c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    Some(out)
}

/// One argv-fallback notice: names and a length, never the value.
struct ArgvNotice {
    service: String,
    account: String,
    bytes: usize,
}

#[derive(Default)]
struct Held {
    /// How many `HeldWarnings` guards are alive on this thread.
    depth: usize,
    notices: Vec<ArgvNotice>,
}

thread_local! {
    static HELD: RefCell<Held> = RefCell::new(Held::default());
}

/// Holds the argv-fallback WARN on this thread until the guard is dropped (§7.3 step 6): a log
/// write can block, and while a refreshed successor exists only in memory nothing that can
/// block may come before it is on disk. While a guard lives, `upsert` records its notice
/// instead of logging it; the outermost guard's drop emits them, in order, with the same text
/// and fields. With no guard a notice is logged at once. It never leaves its thread.
#[must_use]
pub struct HeldWarnings {
    _thread: PhantomData<*const ()>,
}

/// Opens a scope in which this thread holds the argv-fallback WARN (see `HeldWarnings`).
pub fn hold_warnings() -> HeldWarnings {
    HELD.with(|h| h.borrow_mut().depth += 1);
    HeldWarnings {
        _thread: PhantomData,
    }
}

impl HeldWarnings {
    /// Ends the hold now: emits what was held if this is the outermost guard.
    pub fn release(self) {}
}

impl Drop for HeldWarnings {
    fn drop(&mut self) {
        // Only a guard dropped while this thread's locals are torn down finds the slot gone.
        let held = HELD
            .try_with(|h| {
                let mut h = h.borrow_mut();
                h.depth -= 1;
                if h.depth == 0 {
                    std::mem::take(&mut h.notices)
                } else {
                    Vec::new()
                }
            })
            .unwrap_or_default();
        for n in held {
            emit_argv_fallback(&n);
        }
    }
}

/// Logs the notice, or records it while this thread holds warnings.
fn argv_fallback_notice(service: &str, account: &str, bytes: usize) {
    let notice = ArgvNotice {
        service: service.to_owned(),
        account: account.to_owned(),
        bytes,
    };
    let rest = HELD.with(|h| {
        let mut h = h.borrow_mut();
        if h.depth > 0 {
            h.notices.push(notice);
            None
        } else {
            Some(notice)
        }
    });
    if let Some(n) = rest {
        emit_argv_fallback(&n);
    }
}

fn emit_argv_fallback(n: &ArgvNotice) {
    tracing::warn!(
        service = %n.service,
        account = %n.account,
        bytes = n.bytes,
        "a Keychain value is too long for security -i, so it goes on security's command line"
    );
}

impl Keychain for SecurityCli {
    /// A secret read (`-w`), counted by how it ended (D3 change 6): answered once it gives a
    /// secret, locked on rc 36 or a timeout. An absent item (rc 44), which a locked keychain
    /// still reports, and every other failure count as neither.
    fn find(&self, service: &str, account: &str) -> Read<Vec<u8>> {
        let read = match self.run(
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
            locked @ (RunResult::Exited { code: 36, .. } | RunResult::TimedOut) => {
                self.counts.record_locked();
                Read::Unreadable(unreadable(locked))
            }
            other => Read::Unreadable(unreadable(other)),
        };
        if read.is_present() {
            self.counts.record_answered();
        }
        read
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

    /// Never a cancellation point, its verify read included (H4, §14.1).
    fn upsert(&self, service: &str, account: &str, data: &[u8]) -> Result<(), KeychainError> {
        let _write = cancel::uninterruptible();
        check_name(service)?;
        check_name(account)?;
        // The credential as `-w` text when `escape_text` accepts it, else as hex. Past the line
        // limit argv uses the same flag: `-w` with the credential's own (unescaped) text, or
        // `-X` with hex. The hex line is never shorter than the text line, so there is nothing
        // to gain by switching (H5.1).
        let text = escape_text(data);
        let value = match &text {
            Some(escaped) => format!("-w \"{escaped}\""),
            None => format!("-X \"{}\"", hex::encode(data)),
        };
        let line = format!(
            "add-generic-password -U -a \"{account}\" -s \"{service}\" {value}{}\n",
            self.tail()
        );
        let result = if line.len() <= LINE_LIMIT {
            self.run(s(&["-i"]), Some(line.as_bytes()))
        } else {
            argv_fallback_notice(service, account, data.len());
            // argv needs no quoting: text goes as is, never as `escape_text`'s output.
            let (flag, arg) = match text {
                Some(_) => (
                    "-w",
                    std::str::from_utf8(data)
                        .expect("escape_text accepts only UTF-8")
                        .to_owned(),
                ),
                None => ("-X", hex::encode(data)),
            };
            self.run(
                s(&[
                    "add-generic-password",
                    "-U",
                    "-a",
                    account,
                    "-s",
                    service,
                    flag,
                    &arg,
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

    /// Never a cancellation point (H4, §14.1).
    fn delete(&self, service: &str, account: &str) -> Result<(), KeychainError> {
        let _write = cancel::uninterruptible();
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
    /// must then find none. A locked keychain (rc 36) stops it, named as such. Never a
    /// cancellation point, its probe included (H4, §14.1).
    fn delete_service(&self, service: &str) -> Result<u32, KeychainError> {
        let _write = cancel::uninterruptible();
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

    fn read_counts(&self) -> Option<Arc<ReadCounts>> {
        Some(self.counts.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::ReadTally;
    use std::collections::VecDeque;
    use std::fs;
    use std::io;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Instant;

    type Call = (String, Vec<String>, Option<Vec<u8>>);

    #[derive(Default, Clone)]
    struct Scripted {
        calls: Arc<Mutex<Vec<Call>>>,
        results: Arc<Mutex<VecDeque<RunResult>>>,
        /// Whether each `run` was handed a cancel token.
        scoped: Arc<Mutex<Vec<bool>>>,
    }

    impl Scripted {
        fn then(self, r: RunResult) -> Self {
            self.results.lock().unwrap().push_back(r);
            self
        }
        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
        fn scoped(&self) -> Vec<bool> {
            self.scoped.lock().unwrap().clone()
        }
    }

    impl Runner for Scripted {
        fn run(
            &self,
            program: &str,
            args: &[String],
            stdin: Option<&[u8]>,
            _t: Duration,
            cancel: Option<&Cancel>,
        ) -> RunResult {
            self.calls.lock().unwrap().push((
                program.into(),
                args.to_vec(),
                stdin.map(<[u8]>::to_vec),
            ));
            self.scoped.lock().unwrap().push(cancel.is_some());
            // As a real spawn does: a token already set runs nothing.
            if let Some(n) = cancel.and_then(Cancel::requested) {
                return RunResult::Interrupted(n);
            }
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

    /// Keeps a second dispatcher registered for the process. `tracing` caches each callsite's
    /// interest from every registered dispatcher; with only one, it asks just the subscriber of
    /// the thread that reaches the callsite first, and tests on threads with no capture could
    /// cache the upsert WARN as never wanted, losing the event of a capture (as the engine's
    /// `capture_logs` does with `ALWAYS_TWO`).
    static ALWAYS_TWO: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();

    /// The `tracing` lines `f` sends on this thread, level first, without times.
    fn logged(f: impl FnOnce()) -> Vec<String> {
        #[derive(Clone, Default)]
        struct Lines(Arc<Mutex<Vec<u8>>>);
        impl io::Write for Lines {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Lines {
            type Writer = Lines;
            fn make_writer(&'a self) -> Lines {
                self.clone()
            }
        }
        ALWAYS_TWO
            .get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
        let lines = Lines::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(lines.clone())
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::TRACE)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let text = String::from_utf8(lines.0.lock().unwrap().clone()).unwrap();
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn a_text_credential_is_written_as_escaped_w_text_through_interactive_mode() {
        let data = r#"{"k":"a\"b\\c é"}"#.as_bytes(); // not `br`: byte literals are ASCII-only
        let s = Scripted::default().then(ok(b"")).then(ok(data));
        cli(&s, Some("/tmp/t.keychain"))
            .upsert("svc", "acct", data)
            .unwrap();
        let calls = s.calls();
        assert_eq!(calls.len(), 2, "the write, then its verify read");
        assert_eq!(calls[0].1, vec!["-i".to_string()]);
        assert_eq!(
            calls[0].2.as_deref().unwrap(),
            [
                r#"add-generic-password -U -a "acct" -s "svc" -w "{\"k\":\"a\\\"b\\\\c é\"}" "/tmp/t.keychain""#,
                "\n"
            ]
            .concat()
            .as_bytes()
        );
    }

    #[test]
    fn bytes_with_nul_cr_or_lf_or_that_are_not_utf8_keep_the_hex_line() {
        for data in [&b"a\0b"[..], b"a\rb", b"a\nb", &[0xff, 0xfe]] {
            let hex = hex::encode(data);
            let s = Scripted::default()
                .then(ok(b""))
                .then(ok(format!("{hex}\n").as_bytes()))
                .then(og_hex(&hex.to_uppercase()));
            cli(&s, None).upsert("svc", "acct", data).unwrap();
            assert_eq!(
                s.calls()[0].2.as_deref().unwrap(),
                format!("add-generic-password -U -a \"acct\" -s \"svc\" -X \"{hex}\"\n").as_bytes(),
                "{data:?}"
            );
        }
    }

    #[test]
    fn a_text_line_of_exactly_the_limit_stays_on_standard_input() {
        // The line, newline included, is LINE_LIMIT bytes; one more byte takes argv.
        let fixed = "add-generic-password -U -a \"acct\" -s \"svc\" -w \"\"\n".len();
        let at_limit = "x".repeat(LINE_LIMIT - fixed);
        let s = Scripted::default()
            .then(ok(b""))
            .then(ok(at_limit.as_bytes()));
        cli(&s, None)
            .upsert("svc", "acct", at_limit.as_bytes())
            .unwrap();
        assert_eq!(s.calls()[0].2.as_ref().unwrap().len(), LINE_LIMIT);
        let over = format!("{at_limit}x");
        let s = Scripted::default().then(ok(b"")).then(ok(over.as_bytes()));
        cli(&s, None)
            .upsert("svc", "acct", over.as_bytes())
            .unwrap();
        assert!(s.calls()[0].2.is_none(), "argv");
    }

    #[test]
    fn an_over_long_text_credential_goes_on_argv_as_raw_w_text_with_a_warn() {
        let data = format!(
            r#"{{"k":"a\"b\\c","s":"SENTINEL","pad":"{}"}}"#,
            "x".repeat(4100)
        );
        let s = Scripted::default().then(ok(b"")).then(ok(data.as_bytes()));
        let lines = logged(|| {
            cli(&s, None)
                .upsert("svc", "acct", data.as_bytes())
                .unwrap()
        });
        let (_, args, stdin) = &s.calls()[0];
        assert!(stdin.is_none());
        // argv needs no quoting: the text goes unescaped, never through `escape_text`'s output.
        assert_eq!(
            args,
            &argv(
                &[
                    "add-generic-password",
                    "-U",
                    "-a",
                    "acct",
                    "-s",
                    "svc",
                    "-w",
                    &data
                ],
                None
            )
        );
        let warns: Vec<&String> = lines.iter().filter(|l| l.contains("WARN")).collect();
        assert_eq!(warns.len(), 1, "{lines:#?}");
        assert!(
            warns[0].contains(&format!("service=svc account=acct bytes={}", data.len())),
            "{}",
            warns[0]
        );
        assert!(
            lines.iter().all(|l| !l.contains("SENTINEL")),
            "never the value"
        );
    }

    #[test]
    fn an_over_long_binary_credential_goes_on_argv_as_hex_with_a_warn() {
        // Replaces `long_writes_fall_back_to_argv`: 2100 bytes of `x` now fit as text.
        let data = vec![0xff; 2100]; // 4200 hex digits: over the line limit
        let s = Scripted::default().then(ok(b"")).then(ok(&data));
        let lines = logged(|| cli(&s, None).upsert("svc", "acct", &data).unwrap());
        let (_, args, stdin) = &s.calls()[0];
        assert!(stdin.is_none());
        assert_eq!(
            args,
            &argv(
                &[
                    "add-generic-password",
                    "-U",
                    "-a",
                    "acct",
                    "-s",
                    "svc",
                    "-X",
                    &hex::encode(&data)
                ],
                None
            )
        );
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.contains("WARN") && l.contains("bytes=2100"))
                .count(),
            1,
            "{lines:#?}"
        );
    }

    /// An over-long text credential, so `upsert` takes the argv fallback.
    fn long_text() -> String {
        "x".repeat(LINE_LIMIT + 100)
    }

    fn argv_warns(lines: &[String]) -> Vec<&String> {
        lines.iter().filter(|l| l.contains("WARN")).collect()
    }

    #[test]
    fn a_held_argv_warn_is_logged_only_when_the_guard_drops() {
        let data = long_text();
        let s = Scripted::default().then(ok(b"")).then(ok(data.as_bytes()));
        let mut before = vec![];
        let lines = logged(|| {
            let held = hold_warnings();
            cli(&s, None)
                .upsert("svc", "acct", data.as_bytes())
                .unwrap();
            assert_eq!(s.calls().len(), 2, "the write ran");
            before.push(HELD.with(|h| h.borrow().notices.len()));
            drop(held);
        });
        assert_eq!(before, [1], "recorded, not logged, while the guard lived");
        let warns = argv_warns(&lines);
        assert_eq!(warns.len(), 1, "{lines:#?}");
        assert!(
            warns[0].contains(&format!("service=svc account=acct bytes={}", data.len())),
            "{}",
            warns[0]
        );
        assert!(
            warns[0].contains(
                "a Keychain value is too long for security -i, so it goes on security's command line"
            ),
            "{}",
            warns[0]
        );
    }

    #[test]
    fn nothing_is_logged_while_a_guard_lives() {
        let data = long_text();
        let s = Scripted::default().then(ok(b"")).then(ok(data.as_bytes()));
        let mut during = vec![];
        let lines = logged(|| {
            let held = hold_warnings();
            during = logged(|| {
                cli(&s, None)
                    .upsert("svc", "acct", data.as_bytes())
                    .unwrap();
            });
            held.release();
        });
        assert!(during.is_empty(), "{during:#?}");
        assert_eq!(argv_warns(&lines).len(), 1, "{lines:#?}");
    }

    #[test]
    fn nested_guards_emit_once_when_the_outermost_drops_in_order() {
        let data = long_text();
        let s = Scripted::default()
            .then(ok(b""))
            .then(ok(data.as_bytes()))
            .then(ok(b""))
            .then(ok(data.as_bytes()));
        let lines = logged(|| {
            let outer = hold_warnings();
            {
                let inner = hold_warnings();
                cli(&s, None)
                    .upsert("svc", "first", data.as_bytes())
                    .unwrap();
                inner.release();
            }
            cli(&s, None)
                .upsert("svc", "second", data.as_bytes())
                .unwrap();
            assert_eq!(
                HELD.with(|h| h.borrow().notices.len()),
                2,
                "the inner release emitted nothing"
            );
            drop(outer);
        });
        let warns = argv_warns(&lines);
        assert_eq!(warns.len(), 2, "{lines:#?}");
        assert!(warns[0].contains("account=first"), "{}", warns[0]);
        assert!(warns[1].contains("account=second"), "{}", warns[1]);
    }

    #[test]
    fn holding_is_per_thread_and_ends_with_the_guard() {
        let held = hold_warnings();
        // Another thread has no guard, so it logs at once.
        let other = std::thread::spawn(|| {
            let data = long_text();
            let s = Scripted::default().then(ok(b"")).then(ok(data.as_bytes()));
            logged(|| {
                cli(&s, None)
                    .upsert("svc", "acct", data.as_bytes())
                    .unwrap();
            })
        })
        .join()
        .unwrap();
        assert_eq!(argv_warns(&other).len(), 1, "{other:#?}");
        drop(held);
        assert_eq!(HELD.with(|h| h.borrow().depth), 0);
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

    #[test]
    fn a_real_process_reports_its_code_and_both_streams() {
        let _fork = crate::fork_guard();
        let r = ProcessRunner::bounded(
            "/bin/sh",
            &s(&["-c", "printf out; printf err >&2; exit 3"]),
            None,
            Duration::from_secs(5),
            None,
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
        let _fork = crate::fork_guard();
        let started = Instant::now();
        let r = ProcessRunner::bounded(
            "/bin/sh",
            &s(&["-c", "sleep 8 & echo done"]),
            None,
            Duration::from_secs(5),
            None,
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
            ProcessRunner::bounded(
                "/bin/sh",
                &args,
                None,
                Duration::from_secs(5),
                None,
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

    #[test]
    fn secret_reads_are_counted_by_how_they_ended() {
        // D3 change 6: answered (rc 0, a secret returned) and locked (rc 36, or a timeout).
        // Absent (rc 44), any other rc and a spawn failure count as neither.
        let s = Scripted::default()
            .then(ok(b"{\"a\":1}\n"))
            .then(rc(36))
            .then(RunResult::TimedOut)
            .then(rc(44))
            .then(rc(1))
            .then(RunResult::SpawnFailed("x".into()));
        let k = cli(&s, None);
        let counts = k.read_counts().expect("the security driver keeps counts");
        let mut seen = Vec::new();
        for _ in 0..6 {
            let _ = k.find("svc", "acct");
            seen.push(counts.tally());
        }
        let t = |answered, locked| ReadTally { answered, locked };
        assert_eq!(seen, [t(1, 0), t(1, 1), t(1, 2), t(1, 2), t(1, 2), t(1, 2)]);
    }

    #[test]
    fn an_answer_counts_once_the_read_gives_a_secret() {
        // rc 0 whose hex-looking output `-g` confirms is one answered read; one whose `-g`
        // fails gave no secret, so it counts as neither.
        let data = [0xde, 0xad, 0xbe, 0xef];
        let s = Scripted::default()
            .then(ok(format!("{}\n", hex::encode(data)).as_bytes()))
            .then(og_hex("DEADBEEF"))
            .then(ok(b"cafe\n"))
            .then(rc(1));
        let k = cli(&s, None);
        let counts = k.read_counts().unwrap();
        assert!(k.find("svc", "acct").is_present());
        assert_eq!(
            counts.tally(),
            ReadTally {
                answered: 1,
                locked: 0
            }
        );
        assert!(matches!(k.find("svc", "acct"), Read::Unreadable(_)));
        assert_eq!(
            counts.tally(),
            ReadTally {
                answered: 1,
                locked: 0
            }
        );
    }

    #[test]
    fn a_writes_read_back_is_an_answered_secret_read() {
        // A switch writes the live credential and reads it back: a run that switched read a
        // secret (D4.1, D14).
        let s = Scripted::default().then(ok(b"")).then(ok(b"{}\n"));
        let k = cli(&s, None);
        k.upsert("svc", "acct", b"{}").unwrap();
        assert_eq!(
            k.read_counts().unwrap().tally(),
            ReadTally {
                answered: 1,
                locked: 0
            }
        );
    }

    #[test]
    fn attribute_reads_the_lock_check_and_deletes_are_never_counted() {
        // Only `find-generic-password -w` reads a secret (D3 change 6).
        let s = Scripted::default()
            .then(ok(b""))
            .then(rc(36))
            .then(rc(36))
            .then(rc(36))
            .then(rc(36));
        let k = cli(&s, None);
        assert!(k.exists("svc", "acct").is_present());
        assert!(matches!(k.exists("svc", "acct"), Read::Unreadable(_)));
        assert!(matches!(k.service_has_items("svc"), Read::Unreadable(_)));
        assert_eq!(k.lock_state(), LockState::Locked);
        assert!(k.delete("svc", "acct").is_err());
        assert_eq!(k.read_counts().unwrap().tally(), ReadTally::default());
    }

    #[test]
    fn a_locked_oauth_read_beside_an_absent_managed_key_still_looks_locked() {
        // D4.1, D14: a locked keychain still reports an absent item (rc 44), which changes
        // nothing; one answered read in the same run means the keychain is not locked.
        let s = Scripted::default()
            .then(rc(36))
            .then(rc(44))
            .then(rc(36))
            .then(ok(b"sk-ant-api03-abc\n"));
        let k = cli(&s, None);
        let counts = k.read_counts().unwrap();
        let before = counts.tally();
        let _ = k.find("Claude Code-credentials", "me");
        let _ = k.find("Claude Code", "me");
        assert!(counts.tally().since(before).looks_locked());
        let before = counts.tally();
        let _ = k.find("Claude Code-credentials", "me");
        let _ = k.find("Claude Code", "me");
        assert!(!counts.tally().since(before).looks_locked());
    }

    #[test]
    fn reads_take_the_scope_s_token_and_nothing_takes_one_outside_a_scope() {
        let s = Scripted::default()
            .then(ok(b"cafe\n"))
            .then(og_verbatim("cafe")) // find, and its -g
            .then(ok(b""))
            .then(ok(b""))
            .then(ok(b"")) // exists, lock_state, service_has_items
            .then(ok(b"{}\n"))
            .then(ok(b""))
            .then(ok(b""))
            .then(ok(b""));
        let k = cli(&s, None);
        let token = Cancel::new();
        {
            let _scope = cancel::interruptible(&token);
            assert!(k.find("svc", "acct").is_present());
            assert!(k.exists("svc", "acct").is_present());
            assert_eq!(k.lock_state(), LockState::Unlocked);
            assert!(matches!(k.service_has_items("svc"), Read::Present(true)));
        }
        assert!(k.find("svc", "acct").is_present());
        assert!(k.exists("svc", "acct").is_present());
        assert_eq!(k.lock_state(), LockState::Unlocked);
        assert!(matches!(k.service_has_items("svc"), Read::Present(true)));
        assert_eq!(
            s.scoped(),
            [true, true, true, true, true, false, false, false, false]
        );
    }

    #[test]
    fn a_read_whose_scope_holds_a_signal_is_interrupted_and_counted_as_neither() {
        // H4, §14.1. Nothing is scripted: a call that ran would panic.
        let s = Scripted::default();
        let k = cli(&s, None);
        let token = Cancel::new();
        token.request(15);
        let _scope = cancel::interruptible(&token);
        let Read::Unreadable(e) = k.find("svc", "acct") else {
            panic!("not interrupted")
        };
        assert_eq!((e.interruption(), e.is_locked()), (Some(15), false));
        assert!(
            matches!(k.exists("svc", "acct"), Read::Unreadable(e) if e.interruption() == Some(15))
        );
        assert_eq!(k.lock_state(), LockState::Unknown);
        assert_eq!(k.read_counts().unwrap().tally(), ReadTally::default());
    }

    #[test]
    fn a_cancel_set_before_a_write_stops_neither_the_write_nor_its_verify() {
        // H4, H14: upsert (its verify read included), delete and delete_service never take the
        // token, and the scope is whole again after each.
        let s = Scripted::default()
            .then(ok(b""))
            .then(ok(b"{}\n")) // upsert's -i line, its verify read
            .then(rc(44)) // delete
            .then(ok(b""))
            .then(rc(44))
            .then(rc(44)); // delete_service: one, none left, the probe
        let k = cli(&s, None);
        let token = Cancel::new();
        token.request(15);
        let _scope = cancel::interruptible(&token);
        k.upsert("svc", "acct", b"{}").unwrap();
        k.delete("svc", "acct").unwrap();
        assert_eq!(k.delete_service("svc").unwrap(), 1);
        assert_eq!(s.scoped()[..], [false; 6]);
        assert!(
            matches!(k.find("svc", "acct"), Read::Unreadable(e) if e.interruption() == Some(15)),
            "the next read is in the scope again"
        );
    }

    #[test]
    fn rc_36_is_a_locked_read_and_a_timeout_or_any_other_rc_is_not() {
        let s = Scripted::default()
            .then(rc(36))
            .then(RunResult::TimedOut)
            .then(rc(1));
        let k = cli(&s, None);
        let locked = |r: Read<Vec<u8>>| match r {
            Read::Unreadable(e) => e.is_locked(),
            other => panic!("{other:?}"),
        };
        assert!(locked(k.find("s", "a")));
        assert!(!locked(k.find("s", "a")));
        assert!(!locked(k.find("s", "a")));
    }

    /// A shell that leaves a grandchild writing `marker` a second later, then waits 30 s.
    fn with_grandchild(marker: &Path) -> Vec<String> {
        s(&[
            "-c",
            &format!("(sleep 1; echo alive > '{}') & sleep 30", marker.display()),
        ])
    }

    #[test]
    fn a_timeout_or_a_cancel_kills_a_security_child_s_whole_group() {
        // H4: `security`'s own runner killed the child alone and never saw the token.
        let _fork = crate::fork_guard();
        let d = tempfile::tempdir().unwrap();
        let (timed, cancelled) = (d.path().join("timed"), d.path().join("cancelled"));
        let r = ProcessRunner.run(
            "/bin/sh",
            &with_grandchild(&timed),
            None,
            Duration::from_millis(200),
            None,
        );
        assert!(matches!(r, RunResult::TimedOut), "{r:?}");
        let token = Cancel::new();
        let signaller = token.clone();
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            signaller.request(15);
        });
        let started = Instant::now();
        let r = ProcessRunner.run(
            "/bin/sh",
            &with_grandchild(&cancelled),
            None,
            Duration::from_secs(30),
            Some(&token),
        );
        sender.join().unwrap();
        assert!(matches!(r, RunResult::Interrupted(15)), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(1500));
        assert!(
            !timed.exists() && !cancelled.exists(),
            "both grandchildren died with their groups"
        );
    }

    #[test]
    fn stdin_reaches_a_security_child_and_a_signal_s_end_is_rc_minus_one() {
        let _fork = crate::fork_guard();
        let line = b"add-generic-password -U -a \"acct\" -s \"svc\" -X \"00\"\n";
        let r = ProcessRunner.run(
            "/bin/sh",
            &s(&["-c", "cat"]),
            Some(line),
            Duration::from_secs(5),
            None,
        );
        assert!(
            matches!(&r, RunResult::Exited { code: 0, stdout, .. } if stdout == line),
            "{r:?}"
        );
        let r = ProcessRunner.run(
            "/bin/sh",
            &s(&["-c", "kill -KILL $$"]),
            None,
            Duration::from_secs(5),
            None,
        );
        assert!(matches!(r, RunResult::Exited { code: -1, .. }), "{r:?}");
    }

    #[test]
    fn escape_text_escapes_only_backslashes_and_quotes() {
        assert_eq!(
            escape_text(br#"{"a":"b\c"}"#).as_deref(),
            Some(r#"{\"a\":\"b\\c\"}"#)
        );
        // A JSON escape sequence is two characters of text; its backslash is escaped like any other.
        assert_eq!(
            escape_text(br#""x\n\u00e9""#).as_deref(),
            Some(r#"\"x\\n\\u00e9\""#)
        );
        let untouched = "é 日本 🔑 # $ % ' \t end";
        assert_eq!(
            escape_text(untouched.as_bytes()).as_deref(),
            Some(untouched)
        );
    }

    #[test]
    fn escape_text_refuses_nul_cr_lf_and_bytes_that_are_not_utf8() {
        for bytes in [&b"a\0b"[..], b"a\rb", b"a\nb", &[0xff, 0xfe], &[0xc3]] {
            assert_eq!(escape_text(bytes), None, "{bytes:?}");
        }
    }
}
