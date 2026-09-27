use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::keychain::{Keychain, KeychainError, LockState};
use crate::read::{Read, ReadError};

pub const SECURITY: &str = "/usr/bin/security";
/// The 4096-byte `security -i` line limit minus 64. An over-long line truncates silently and
/// leaves the old entry (Appendix A.3).
pub const LINE_LIMIT: usize = 4032;
const TIMEOUT: Duration = Duration::from_secs(5);

/// `stdout` of `find-generic-password -w` is a secret, so `Debug` shows lengths only.
#[derive(Clone)]
pub enum RunResult {
    Exited {
        code: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
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
    fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> RunResult;
    /// Runs with the terminal attached: stdin and stderr inherited, the child's stdout sent to
    /// stderr (stdout is reserved for command output, §14). No timeout, because a person is
    /// answering. `Exited` carries no output.
    fn run_attached(&self, program: &str, args: &[String]) -> RunResult;
}

pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> RunResult {
        let mut child = match Command::new(program)
            .args(args)
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
        let drain = |p: Option<Box<dyn std::io::Read + Send>>| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                if let Some(mut p) = p {
                    let _ = p.read_to_end(&mut buf);
                }
                buf
            })
        };
        let out = drain(child.stdout.take().map(|p| Box::new(p) as _));
        let err = drain(child.stderr.take().map(|p| Box::new(p) as _));
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return RunResult::Exited {
                        code: status.code().unwrap_or(-1),
                        stdout: out.join().unwrap_or_default(),
                        stderr: err.join().unwrap_or_default(),
                    };
                }
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return RunResult::TimedOut;
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(e) => return RunResult::SpawnFailed(e.to_string()),
            }
        }
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

/// Exactly one trailing `\n` is stripped; all-lowercase-hex output of even length means the
/// stored data was non-printable, so it is decoded.
pub fn decode_output(mut out: Vec<u8>) -> Vec<u8> {
    if out.last() == Some(&b'\n') {
        out.pop();
    }
    let hexlike = !out.is_empty()
        && out.len() % 2 == 0
        && out
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b));
    if hexlike {
        if let Ok(decoded) = hex::decode(&out) {
            return decoded;
        }
    }
    out
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
}

impl Default for SecurityCli {
    fn default() -> Self {
        Self::new()
    }
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_owned()).collect()
}

fn unreadable(r: RunResult) -> ReadError {
    match r {
        RunResult::Exited { code, stderr, .. } => ReadError::new(
            "keychain",
            format!("rc {code}: {}", String::from_utf8_lossy(&stderr).trim()),
        ),
        RunResult::TimedOut => ReadError::new("keychain", "security timed out after 5 s"),
        RunResult::SpawnFailed(e) => {
            ReadError::new("keychain", format!("could not run security: {e}"))
        }
    }
}

fn failed(r: RunResult) -> KeychainError {
    match r {
        RunResult::Exited { code, stderr, .. } => KeychainError {
            rc: Some(code),
            detail: String::from_utf8_lossy(&stderr).trim().to_owned(),
        },
        RunResult::TimedOut => KeychainError {
            rc: None,
            detail: "security timed out after 5 s".into(),
        },
        RunResult::SpawnFailed(e) => KeychainError {
            rc: None,
            detail: format!("could not run security: {e}"),
        },
    }
}

fn check_name(v: &str) -> Result<(), KeychainError> {
    if v.contains(['"', '\\', '\n']) {
        return Err(KeychainError {
            rc: None,
            detail: format!("refusing an item name with quotes: {v:?}"),
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
            } => Read::Present(decode_output(stdout)),
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

    #[test]
    fn hex_output_is_decoded() {
        assert_eq!(decode_output(b"7b7d\n".to_vec()), b"{}");
        assert_eq!(
            decode_output(b"sk-ant-api03-abc\n".to_vec()),
            b"sk-ant-api03-abc"
        );
        assert_eq!(decode_output(b"abc\n".to_vec()), b"abc"); // odd length: not hex
    }

    #[test]
    fn find_maps_return_codes() {
        let s = Scripted::default()
            .then(rc(44))
            .then(rc(36))
            .then(RunResult::TimedOut)
            .then(RunResult::SpawnFailed("x".into()));
        let k = cli(&s, None);
        assert!(matches!(k.find("s", "a"), Read::Absent));
        assert!(matches!(k.find("s", "a"), Read::Unreadable(e) if e.detail.contains("rc 36")));
        assert!(matches!(k.find("s", "a"), Read::Unreadable(e) if e.detail.contains("timed out")));
        assert!(matches!(k.find("s", "a"), Read::Unreadable(_)));
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
        let s = Scripted::default().then(ok(b"")).then(ok(b"7b7d\n"));
        cli(&s, None).upsert("svc", "acct", b"{}").unwrap();
        let calls = s.calls();
        assert_eq!(calls[0].1, vec!["-i".to_string()]);
        assert_eq!(
            calls[0].2.as_deref().unwrap(),
            b"add-generic-password -U -a \"acct\" -s \"svc\" -X \"7b7d\"\n"
        );
        assert_eq!(calls[1].1[0], "find-generic-password");
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
    fn the_lock_check_maps_show_keychain_info() {
        let s = Scripted::default()
            .then(ok(b""))
            .then(rc(36))
            .then(rc(128))
            .then(RunResult::TimedOut);
        let k = cli(&s, None);
        assert_eq!(k.lock_state(), LockState::Unlocked);
        assert_eq!(k.lock_state(), LockState::Locked);
        assert_eq!(k.lock_state(), LockState::Unknown); // rc 128: a locked keychain file
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
}
