use std::collections::VecDeque;
use std::ffi::{CString, OsStr, OsString};
use std::fs;
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::cancel::Cancel;

/// A pid plus its start time, so a recycled pid is never mistaken for the original (§12.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessStamp {
    pub pid: u32,
    pub start: u64,
}

impl ProcessStamp {
    pub fn current() -> io::Result<Self> {
        let pid = std::process::id();
        let start = start_of(pid)?.ok_or_else(|| io::Error::other("own process not found"))?;
        Ok(Self { pid, start })
    }

    /// Exact pid and start-time match. Anything undeterminable counts as live.
    pub fn is_live(&self) -> bool {
        match start_of(self.pid) {
            Ok(Some(start)) => start == self.start,
            Ok(None) => false,
            Err(_) => true,
        }
    }
}

/// `/proc/<pid>/stat` field 22, counted after the last `)`.
#[cfg(target_os = "linux")]
pub(crate) fn start_of(pid: u32) -> io::Result<Option<u64>> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let after = stat
        .rfind(')')
        .map(|i| &stat[i + 1..])
        .ok_or_else(|| io::Error::other("bad stat"))?;
    // `after` starts at field 3; field 22 is the 20th item.
    after
        .split_whitespace()
        .nth(19)
        .and_then(|f| f.parse().ok())
        .map(Some)
        .ok_or_else(|| io::Error::other("bad stat"))
}

/// `proc_pidinfo(PROC_PIDTBSDINFO)` start time, in microseconds.
#[cfg(target_os = "macos")]
pub(crate) fn start_of(pid: u32) -> io::Result<Option<u64>> {
    // SAFETY: `proc_bsdinfo` is a C struct of plain integers; the zeroed value is a valid
    // bit pattern for it and is fully overwritten by `proc_pidinfo` below on success.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is a correctly sized, writable proc_bsdinfo.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if n == size {
        return Ok(Some(
            info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
        ));
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::ESRCH) {
        Ok(None)
    } else {
        Err(e)
    }
}

/// What to run, and the environment and directory to run it in (§12.3 step 8, §12.5). The
/// child starts from this process's environment: `remove` is applied first, then `set`, so a
/// name in both ends up set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub set: Vec<(OsString, OsString)>,
    pub remove: Vec<OsString>,
    pub cwd: Option<PathBuf>,
}

/// How a captured child ended (`run_captured`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    /// It exited, and both its pipes closed. `code` is its exit code, or `signal` the signal
    /// that ended it.
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// It ran past the timeout and its group was killed, or it exited but its output stayed
    /// open past the drain grace (a grandchild held a pipe): half an output is never returned.
    TimedOut,
    /// The cancel token was set: its group was killed, or it was never spawned.
    Interrupted(i32),
    /// It could not be spawned, or its wait failed.
    SpawnFailed(String),
}

/// How often a captured child's wait looks at its exit, the deadline and the token.
const POLL: Duration = Duration::from_millis(10);
/// How long `run_captured` waits for the output pipes to close once the child has exited, as
/// `security`'s runner does: a grandchild that inherited a pipe keeps it open after the child.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// The `Command` for `spec`: its program, arguments, environment and directory.
fn command(spec: &SpawnSpec) -> Command {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args);
    for name in &spec.remove {
        cmd.env_remove(name);
    }
    for (name, value) in &spec.set {
        cmd.env(name, value);
    }
    if let Some(dir) = &spec.cwd {
        cmd.current_dir(dir);
    }
    cmd
}

/// Non-interactive: its own process group, stdin null, stdout and stderr captured, the group
/// killed (`killpg`) on `timeout` or when `cancel` is set (polled every 10 ms). A token already
/// set spawns nothing (§12.5: the wait for the login check is a cancellation point).
pub fn run_captured(spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured {
    capture(spec, timeout, cancel, DRAIN_GRACE)
}

/// `run_captured`, with the drain grace a parameter so the tests need not wait 2 s.
fn capture(spec: &SpawnSpec, timeout: Duration, cancel: &Cancel, grace: Duration) -> Captured {
    if let Some(signal) = cancel.requested() {
        return Captured::Interrupted(signal);
    }
    let mut cmd = command(spec);
    // §14.1: a group of its own, so a Ctrl-C at the terminal never reaches it, and a timeout
    // or a cancel kills whatever it started too. Its stdin is null and its output is piped, so
    // the background group never stops it for touching the terminal.
    cmd.process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Captured::SpawnFailed(e.to_string()),
    };
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill_group(&mut child);
                return Captured::SpawnFailed(e.to_string());
            }
        }
        if let Some(signal) = cancel.requested() {
            kill_group(&mut child);
            return Captured::Interrupted(signal);
        }
        if Instant::now() >= deadline {
            kill_group(&mut child);
            return Captured::TimedOut;
        }
        thread::sleep(POLL);
    };
    // The child is reaped now, so its group id may name another group once its last member
    // goes: whatever still holds a pipe is left alone rather than killed by a stale id.
    let until = Instant::now() + grace;
    let output =
        collect(&out, until, cancel).and_then(|stdout| Ok((stdout, collect(&err, until, cancel)?)));
    match output {
        Ok((stdout, stderr)) => Captured::Exited {
            code: status.code(),
            signal: status.signal(),
            stdout,
            stderr,
        },
        Err(Unfinished::Late) => Captured::TimedOut,
        Err(Unfinished::Interrupted(signal)) => Captured::Interrupted(signal),
    }
}

/// Kills the child's whole group, then reaps the child. The child leads the group
/// (`process_group(0)`) and is not reaped yet, so its pid still names that group and no other.
/// A kill that fails leaves the child running, and `wait` would block for as long as it does,
/// so then it returns without waiting (as `security`'s runner does).
fn kill_group(child: &mut Child) {
    let group = child.id() as libc::pid_t;
    // SAFETY: killpg(2) takes two integers and touches no memory of ours.
    let sent = unsafe { libc::killpg(group, libc::SIGKILL) } == 0;
    if sent || child.kill().is_ok() {
        let _ = child.wait();
    }
}

/// Reads a pipe to its end on a thread and sends what it read. The thread is detached, so a
/// pipe that never closes costs one parked thread, not a hang.
fn drain<R: io::Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });
    rx
}

/// Why a pipe's bytes are not all there.
enum Unfinished {
    Late,
    Interrupted(i32),
}

/// The drained bytes, once the pipe closes by `until`. The wait is sliced, so the token is
/// checked here as in the child's own wait.
fn collect(
    rx: &mpsc::Receiver<Vec<u8>>,
    until: Instant,
    cancel: &Cancel,
) -> Result<Vec<u8>, Unfinished> {
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left.min(POLL)) {
            Ok(bytes) => return Ok(bytes),
            Err(RecvTimeoutError::Disconnected) => return Err(Unfinished::Late),
            Err(RecvTimeoutError::Timeout) => {}
        }
        if let Some(signal) = cancel.requested() {
            return Err(Unfinished::Interrupted(signal));
        }
        if Instant::now() >= until {
            return Err(Unfinished::Late);
        }
    }
}

/// The session: tagteam's foreground process group, stdio inherited, and exactly `inherit`
/// passed without `FD_CLOEXEC` (cleared in `pre_exec`, Decision 3). Every other descriptor
/// tagteam holds is `O_CLOEXEC` (std's default), so no other child ever holds a reservation.
/// `spawn` returns once the child has exec'd, so a signal sent to its pid after that reaches
/// the launch command itself.
pub fn spawn_session(spec: &SpawnSpec, inherit: Option<RawFd>) -> io::Result<Child> {
    let mut cmd = command(spec);
    if let Some(fd) = inherit {
        // SAFETY: the closure runs in the forked child before exec, where only
        // async-signal-safe work is allowed: it makes two fcntl(2) calls on an integer and
        // reads errno, and allocates nothing.
        unsafe {
            cmd.pre_exec(move || keep_across_exec(fd));
        }
    }
    cmd.spawn()
}

/// Clears `FD_CLOEXEC` on `fd`, in the child (`spawn_session`).
fn keep_across_exec(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl(2) with F_GETFD takes and returns integers.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above, with F_SETFD.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The plain path (§12.1): `exec`; returns only the error. On success this process becomes the
/// launch command, with its pid, its terminal and its exit status.
pub fn exec_command(spec: &SpawnSpec) -> io::Error {
    command(spec).exec()
}

/// `PATH` lookup of `name`: the first executable regular file, as a shell finds it (§12.1).
/// A name with a `/` is not searched for, and an empty `PATH` entry is the current directory.
/// The result is absolute unless `name` itself was a relative path.
pub fn find_on_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if name.contains('/') {
        let p = PathBuf::from(name);
        return executable_file(&p).then_some(p);
    }
    for dir in path?.as_bytes().split(|b| *b == b':') {
        let dir = if dir.is_empty() {
            Path::new(".")
        } else {
            Path::new(OsStr::from_bytes(dir))
        };
        let mut candidate = dir.join(name);
        if candidate.is_relative() {
            match std::env::current_dir() {
                Ok(cwd) => candidate = cwd.join(candidate),
                Err(_) => continue,
            }
        }
        if executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// A regular file (symlinks followed) this user may execute, as `access(X_OK)` decides.
fn executable_file(p: &Path) -> bool {
    if !fs::metadata(p).is_ok_and(|m| m.is_file()) {
        return false;
    }
    let Ok(c) = CString::new(p.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a NUL-terminated path that outlives the call; access(2) only reads it.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

/// §12.5: the child's code, `128 + signal` when a signal ended it.
pub fn exit_code(status: ExitStatus) -> i32 {
    match (status.code(), status.signal()) {
        (Some(code), _) => code,
        (None, Some(signal)) => 128 + signal,
        (None, None) => 1,
    }
}

/// The port engine tests script (Decision 10). `SystemSpawner` calls `run_captured`.
pub trait ProcessSpawner: Send + Sync {
    fn run_captured(&self, spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSpawner;

impl ProcessSpawner for SystemSpawner {
    fn run_captured(&self, spec: &SpawnSpec, timeout: Duration, cancel: &Cancel) -> Captured {
        run_captured(spec, timeout, cancel)
    }
}

/// Tests: replies in order, recording each spec. A token already set answers `Interrupted`, as
/// a real spawn does, and leaves the next reply queued; with no reply left the spawn fails, as
/// a missing launch command would.
#[derive(Default)]
pub struct ScriptedSpawner {
    replies: Mutex<VecDeque<Captured>>,
    specs: Mutex<Vec<SpawnSpec>>,
}

impl ScriptedSpawner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, c: Captured) {
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(c);
    }

    pub fn specs(&self) -> Vec<SpawnSpec> {
        self.specs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl ProcessSpawner for ScriptedSpawner {
    fn run_captured(&self, spec: &SpawnSpec, _timeout: Duration, cancel: &Cancel) -> Captured {
        self.specs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(spec.clone());
        if let Some(signal) = cancel.requested() {
            return Captured::Interrupted(signal);
        }
        self.replies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| Captured::SpawnFailed("no reply scripted".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::MutexGuard;

    #[test]
    fn this_process_is_live_and_a_changed_start_is_not() {
        let me = ProcessStamp::current().unwrap();
        assert!(me.is_live());
        assert!(
            !ProcessStamp {
                start: me.start + 1,
                ..me
            }
            .is_live()
        );
    }

    #[test]
    fn an_exited_child_is_not_live() {
        let _fork = crate::FORK_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let stamp = ProcessStamp {
            pid: child.id(),
            start: 0,
        };
        child.wait().unwrap();
        assert!(!stamp.is_live());
    }

    fn fork_guard() -> MutexGuard<'static, ()> {
        crate::FORK_GUARD
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn spec(program: &str, args: &[&str]) -> SpawnSpec {
        SpawnSpec {
            program: PathBuf::from(program),
            args: args.iter().map(OsString::from).collect(),
            ..SpawnSpec::default()
        }
    }

    /// `/bin/sh -c script`.
    fn sh(script: &str) -> SpawnSpec {
        spec("/bin/sh", &["-c", script])
    }

    /// A shell that leaves a background grandchild writing `marker` a second later, then waits
    /// 30 s: whatever kills only the shell lets the grandchild write.
    fn with_grandchild(marker: &Path) -> SpawnSpec {
        sh(&format!(
            "(sleep 1; echo alive > '{}') & sleep 30",
            marker.display()
        ))
    }

    /// Waits for a shell to write its pid to `file`.
    fn pid_in(file: &Path) -> libc::pid_t {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let written = fs::read_to_string(file).ok();
            if let Some(pid) = written.and_then(|t| t.trim().parse().ok()) {
                return pid;
            }
            assert!(Instant::now() < deadline, "the shell never wrote its pid");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn own_group() -> libc::pid_t {
        // SAFETY: getpgrp(2) takes no arguments and cannot fail.
        unsafe { libc::getpgrp() }
    }

    fn group_of(pid: libc::pid_t) -> libc::pid_t {
        // SAFETY: getpgid(2) only reads the process table; `pid` is a child that is still
        // running and not yet reaped, so it names that child and no other process.
        let group = unsafe { libc::getpgid(pid) };
        assert!(group > 0, "getpgid: {}", io::Error::last_os_error());
        group
    }

    #[test]
    fn a_captured_child_returns_its_code_and_both_outputs() {
        let _fork = fork_guard();
        let got = run_captured(
            &sh("printf out; printf err >&2; exit 3"),
            Duration::from_secs(5),
            &Cancel::new(),
        );
        assert_eq!(
            got,
            Captured::Exited {
                code: Some(3),
                signal: None,
                stdout: b"out".to_vec(),
                stderr: b"err".to_vec(),
            }
        );
    }

    #[test]
    fn a_captured_child_killed_by_a_signal_reports_the_signal() {
        let _fork = fork_guard();
        let got = run_captured(&sh("kill -TERM $$"), Duration::from_secs(5), &Cancel::new());
        assert!(
            matches!(
                got,
                Captured::Exited {
                    code: None,
                    signal: Some(15),
                    ..
                }
            ),
            "{got:?}"
        );
    }

    #[test]
    fn a_captured_child_gets_the_spec_s_environment_and_directory() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let env = SpawnSpec {
            set: vec![("TAGTEAM_PROBE".into(), "on".into())],
            remove: vec!["PATH".into(), "TAGTEAM_PROBE".into()],
            ..spec("/usr/bin/env", &[])
        };
        let Captured::Exited {
            code: Some(0),
            stdout,
            ..
        } = run_captured(&env, Duration::from_secs(5), &Cancel::new())
        else {
            panic!("env did not run");
        };
        let lines: Vec<&str> = std::str::from_utf8(&stdout).unwrap().lines().collect();
        assert!(
            lines.contains(&"TAGTEAM_PROBE=on"),
            "set wins over remove: {lines:?}"
        );
        assert!(!lines.iter().any(|l| l.starts_with("PATH=")), "{lines:?}");
        let pwd = SpawnSpec {
            cwd: Some(d.path().to_path_buf()),
            ..sh("pwd -P")
        };
        let Captured::Exited { stdout, .. } =
            run_captured(&pwd, Duration::from_secs(5), &Cancel::new())
        else {
            panic!("pwd did not run");
        };
        let canonical = fs::canonicalize(d.path()).unwrap();
        assert_eq!(stdout, format!("{}\n", canonical.display()).into_bytes());
    }

    #[test]
    fn a_captured_child_reads_no_terminal() {
        // stdin is /dev/null: a child that reads it sees end of file at once.
        let _fork = fork_guard();
        let got = run_captured(&sh("cat"), Duration::from_secs(5), &Cancel::new());
        assert!(
            matches!(&got, Captured::Exited { code: Some(0), stdout, .. } if stdout.is_empty()),
            "{got:?}"
        );
    }

    #[test]
    fn a_captured_child_leads_a_process_group_of_its_own() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("pid");
        let script = sh(&format!("echo $$ > '{}'; sleep 1", file.display()));
        let run =
            thread::spawn(move || run_captured(&script, Duration::from_secs(5), &Cancel::new()));
        let pid = pid_in(&file);
        let group = group_of(pid);
        assert!(matches!(
            run.join().unwrap(),
            Captured::Exited { code: Some(0), .. }
        ));
        assert_eq!(group, pid, "the child leads its own group");
        assert_ne!(
            group,
            own_group(),
            "never the caller's, the terminal's foreground group"
        );
    }

    #[test]
    fn a_timeout_kills_the_child_s_whole_group() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("alive");
        let started = Instant::now();
        let got = run_captured(
            &with_grandchild(&marker),
            Duration::from_millis(200),
            &Cancel::new(),
        );
        assert_eq!(got, Captured::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "not the shell's 30 s"
        );
        thread::sleep(Duration::from_millis(1500));
        assert!(!marker.exists(), "the grandchild died with its group");
    }

    #[test]
    fn a_cancel_kills_the_child_s_whole_group_and_names_the_signal() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("alive");
        let cancel = Cancel::new();
        let signaller = cancel.clone();
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            signaller.request(15);
        });
        let started = Instant::now();
        let got = run_captured(&with_grandchild(&marker), Duration::from_secs(30), &cancel);
        sender.join().unwrap();
        assert_eq!(got, Captured::Interrupted(15));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            cancel.requested(),
            Some(15),
            "the wait reads the token, never takes it"
        );
        thread::sleep(Duration::from_millis(1500));
        assert!(!marker.exists(), "the grandchild died with its group");
    }

    #[test]
    fn a_token_already_set_spawns_nothing() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("ran");
        let cancel = Cancel::new();
        cancel.request(2);
        let got = run_captured(
            &sh(&format!("echo ran > '{}'", marker.display())),
            Duration::from_secs(5),
            &cancel,
        );
        assert_eq!(got, Captured::Interrupted(2));
        thread::sleep(Duration::from_millis(300));
        assert!(!marker.exists());
    }

    #[test]
    fn a_program_that_cannot_be_spawned_is_a_spawn_failure() {
        let _fork = fork_guard();
        let got = run_captured(
            &spec("/nonexistent/tagteam-probe", &[]),
            Duration::from_secs(5),
            &Cancel::new(),
        );
        assert!(matches!(got, Captured::SpawnFailed(_)), "{got:?}");
    }

    #[test]
    fn output_held_open_past_the_grace_is_never_half_reported() {
        // The shell exits at once, but a grandchild keeps its stdout open for 2 s.
        let _fork = fork_guard();
        let got = capture(
            &sh("sleep 2 & printf partial"),
            Duration::from_secs(5),
            &Cancel::new(),
            Duration::from_millis(200),
        );
        assert_eq!(got, Captured::TimedOut);
    }

    #[test]
    fn a_cancel_while_the_output_drains_is_an_interruption() {
        let _fork = fork_guard();
        let cancel = Cancel::new();
        let signaller = cancel.clone();
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            signaller.request(1);
        });
        let started = Instant::now();
        let got = capture(
            &sh("sleep 3 & printf partial"),
            Duration::from_secs(5),
            &cancel,
            Duration::from_secs(5),
        );
        sender.join().unwrap();
        assert_eq!(got, Captured::Interrupted(1));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_session_passes_exactly_the_one_descriptor_it_is_given() {
        let _fork = fork_guard();
        let (keep, other) = (
            File::open("/dev/null").unwrap(),
            File::open("/dev/null").unwrap(),
        );
        let (k, o) = (keep.as_raw_fd(), other.as_raw_fd());
        let inherited = sh(&format!("test -e /dev/fd/{k} && ! test -e /dev/fd/{o}"));
        let status = spawn_session(&inherited, Some(k)).unwrap().wait().unwrap();
        assert!(status.success(), "the child sees {k} and only {k}");
        // SAFETY: fcntl(2) with F_GETFD on a descriptor `keep` owns.
        let flags = unsafe { libc::fcntl(k, libc::F_GETFD) };
        assert_ne!(
            flags & libc::FD_CLOEXEC,
            0,
            "only the child's copy lost FD_CLOEXEC"
        );
        let none = sh(&format!("! test -e /dev/fd/{k} && ! test -e /dev/fd/{o}"));
        let status = spawn_session(&none, None).unwrap().wait().unwrap();
        assert!(
            status.success(),
            "without `inherit`, no descriptor reaches the child"
        );
    }

    #[test]
    fn a_session_with_a_closed_descriptor_is_not_spawned() {
        let _fork = fork_guard();
        let err = spawn_session(&sh("exit 0"), Some(10_000)).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EBADF));
    }

    #[test]
    fn a_session_stays_in_the_caller_s_process_group() {
        // §14.1: `claude` stays in the terminal's foreground group, so a Ctrl-C reaches it.
        let _fork = fork_guard();
        let mut child = spawn_session(&spec("/bin/sleep", &["5"]), None).unwrap();
        let group = group_of(child.id() as libc::pid_t);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(group, own_group());
    }

    #[test]
    fn a_session_gets_the_spec_s_environment_and_directory() {
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let canonical = fs::canonicalize(d.path()).unwrap();
        let session = SpawnSpec {
            set: vec![("TAGTEAM_PROBE".into(), "on".into())],
            cwd: Some(d.path().to_path_buf()),
            ..sh(&format!(
                "test \"$TAGTEAM_PROBE\" = on && test \"$(pwd -P)\" = '{}'",
                canonical.display()
            ))
        };
        let status = spawn_session(&session, None).unwrap().wait().unwrap();
        assert!(status.success());
    }

    /// Set in the copy of this test binary that the exec test starts, naming its scratch dir.
    const EXEC_PROBE: &str = "TAGTEAM_TEST_EXEC_PROBE";

    #[test]
    fn exec_replaces_the_process_and_returns_only_its_error() {
        if let Some(dir) = std::env::var_os(EXEC_PROBE) {
            // The copy. A program that cannot run returns the error, and the process carries
            // on; one that can replaces it, keeping its pid.
            let dir = PathBuf::from(dir);
            let missing = exec_command(&spec("/nonexistent/tagteam-probe", &[]));
            fs::write(dir.join("missing"), format!("{:?}", missing.kind())).unwrap();
            let replaced = SpawnSpec {
                set: vec![("TAGTEAM_PROBE".into(), "on".into())],
                cwd: Some(dir.clone()),
                ..sh("echo \"$$ $TAGTEAM_PROBE $(pwd -P)\" > exec")
            };
            let err = exec_command(&replaced);
            panic!("exec returned: {err}");
        }
        let _fork = fork_guard();
        let d = tempfile::tempdir().unwrap();
        let mut copy = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process::tests::exec_replaces_the_process_and_returns_only_its_error",
                "--test-threads=1",
            ])
            .env(EXEC_PROBE, d.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = copy.id();
        let status = copy.wait().unwrap();
        assert!(status.success(), "{status:?}");
        assert_eq!(
            fs::read_to_string(d.path().join("missing")).unwrap(),
            "NotFound"
        );
        let canonical = fs::canonicalize(d.path()).unwrap();
        assert_eq!(
            fs::read_to_string(d.path().join("exec")).unwrap(),
            format!("{pid} on {}\n", canonical.display()),
            "the shell ran as the copy itself, with the spec's environment and directory"
        );
    }

    #[test]
    fn the_first_executable_regular_file_on_path_is_found() {
        let d = tempfile::tempdir().unwrap();
        let dir = |name: &str| {
            let p = d.path().join(name);
            fs::create_dir_all(&p).unwrap();
            p
        };
        let file = |path: &Path, mode: u32| {
            fs::write(path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        };
        let (plain, holder, real, first, linked) = (
            dir("plain"),
            dir("holder"),
            dir("real"),
            dir("first"),
            dir("linked"),
        );
        file(&plain.join("claude"), 0o644);
        fs::create_dir(holder.join("claude")).unwrap();
        file(&real.join("claude"), 0o755);
        file(&first.join("claude"), 0o755);
        symlink(real.join("claude"), linked.join("claude")).unwrap();
        let path = |dirs: &[&Path]| {
            let joined: Vec<String> = dirs.iter().map(|p| p.display().to_string()).collect();
            OsString::from(joined.join(":"))
        };
        // Neither a file without the execute bit nor a directory is a match; an empty entry
        // (the current directory, which holds no `claude`) is passed over.
        let p = path(&[&plain, &holder, Path::new(""), &real]);
        assert_eq!(find_on_path("claude", Some(&p)), Some(real.join("claude")));
        let p = path(&[&first, &real]);
        assert_eq!(
            find_on_path("claude", Some(&p)),
            Some(first.join("claude")),
            "PATH order"
        );
        let p = path(&[&linked]);
        assert_eq!(
            find_on_path("claude", Some(&p)),
            Some(linked.join("claude")),
            "a link to an executable counts, as itself"
        );
        assert_eq!(find_on_path("nothing-here", Some(&p)), None);
        assert_eq!(find_on_path("claude", None), None, "no PATH, nothing found");
        assert_eq!(find_on_path("", Some(&p)), None);
        assert_eq!(
            find_on_path("/bin/sh", Some(OsStr::new(""))),
            Some(PathBuf::from("/bin/sh")),
            "a name with a slash is taken as it is"
        );
        let unexecutable = plain.join("claude").display().to_string();
        assert_eq!(find_on_path(&unexecutable, Some(&p)), None);
    }

    #[test]
    fn the_exit_code_is_the_child_s_or_128_plus_the_signal() {
        assert_eq!(exit_code(ExitStatus::from_raw(0)), 0);
        assert_eq!(exit_code(ExitStatus::from_raw(3 << 8)), 3);
        assert_eq!(exit_code(ExitStatus::from_raw(libc::SIGTERM)), 143);
        assert_eq!(exit_code(ExitStatus::from_raw(libc::SIGINT)), 130);
        let _fork = fork_guard();
        let killed = Command::new("/bin/sh")
            .args(["-c", "kill -KILL $$"])
            .status()
            .unwrap();
        assert_eq!(exit_code(killed), 137);
    }

    #[test]
    fn the_system_spawner_runs_the_spec() {
        let _fork = fork_guard();
        let got =
            SystemSpawner.run_captured(&sh("printf ok"), Duration::from_secs(5), &Cancel::new());
        assert!(
            matches!(&got, Captured::Exited { code: Some(0), stdout, .. } if stdout == b"ok"),
            "{got:?}"
        );
    }

    #[test]
    fn the_scripted_spawner_replies_in_order_and_records_every_spec() {
        let s = ScriptedSpawner::new();
        let reply = Captured::Exited {
            code: Some(0),
            signal: None,
            stdout: b"{}".to_vec(),
            stderr: vec![],
        };
        s.push(Captured::TimedOut);
        s.push(reply.clone());
        let (one, two) = (
            sh("one"),
            SpawnSpec {
                cwd: Some("/w".into()),
                ..sh("two")
            },
        );
        let (timeout, quiet) = (Duration::from_secs(10), Cancel::new());
        assert_eq!(s.run_captured(&one, timeout, &quiet), Captured::TimedOut);
        assert_eq!(s.run_captured(&two, timeout, &quiet), reply);
        assert!(
            matches!(
                s.run_captured(&one, timeout, &quiet),
                Captured::SpawnFailed(_)
            ),
            "nothing scripted is a spawn failure"
        );
        s.push(Captured::TimedOut);
        let cancelled = Cancel::new();
        cancelled.request(15);
        assert_eq!(
            s.run_captured(&two, timeout, &cancelled),
            Captured::Interrupted(15),
            "a set token wins, as for a real spawn"
        );
        assert_eq!(
            s.run_captured(&one, timeout, &quiet),
            Captured::TimedOut,
            "and leaves the reply queued"
        );
        assert_eq!(
            s.specs(),
            vec![one.clone(), two.clone(), one.clone(), two, one]
        );
    }
}
