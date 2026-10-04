//! §12.5: `tagteam run` once its launch is made. The spawn, the wait, the signals forwarded to
//! `claude`, the exit handling its exit starts, and the exit code (§14.1, B.63).

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::Duration;

use tagteam_engine::launch::{LaunchEnd, Launched};
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::Cancel;
use tagteam_provider::process::{SpawnSpec, exit_code, spawn_session};

use crate::signals;

/// How often the wait loop looks at `claude` and at the cancel token (Decision 1).
const POLL: Duration = Duration::from_millis(50);

/// `run`'s exit code when it lost track of `claude`'s status, which only a failing `waitpid`
/// can cause.
const LOST: i32 = 1;

/// §12.5 steps 6–7 and "When the child exits", after `launch` and the login check. Spawns
/// `launch` with `args` in `cwd` and `launched`'s environment, with the reservation inherited
/// through its fd. Waits for it, forwarding signals (Decision 1), then runs the exit handling
/// and prints its notices on `err`.
///
/// `Ok` is `claude`'s exit code, `128 + n` when signal `n` ended it, whatever exit handling did
/// (B.63). `Err` is a launch that never started: interrupted at the token's last look, or a
/// spawn that failed. Its exit handling has already run (§12.3).
pub(crate) fn run_session(
    engine: &Engine,
    launched: Launched,
    launch: &Path,
    args: &[OsString],
    cwd: &Path,
    cancel: &Cancel,
    err: &mut dyn Write,
) -> Result<i32, EngineError> {
    // Decision 1: Ctrl-\ is the child's. Registered before the spawn, so the exec gives
    // `claude` the default action back.
    if let Err(e) = signals::survive_quit() {
        let _ = writeln!(
            err,
            "warning: Ctrl-\\ may stop tagteam before the session ends: {e}"
        );
    }
    // §12.5: the token's last look. A signal recorded after it is `claude`'s, and goes to it as
    // soon as it exists.
    if let Some(signal) = cancel.take() {
        notify(err, &engine.finish_run(launched, LaunchEnd::Refused));
        return Err(EngineError::Interrupted(signal));
    }
    pause_point("before-spawn");
    let spec = SpawnSpec {
        program: launch.to_path_buf(),
        args: args.to_vec(),
        set: launched.env.set.clone(),
        remove: launched.env.remove.clone(),
        cwd: Some(cwd.to_path_buf()),
    };
    let mut child = match spawn_session(&spec, Some(launched.reservation.fd())) {
        Ok(child) => child,
        Err(e) => {
            let e = EngineError::LaunchUnreachable {
                detail: format!("{}: {e}", launch.display()),
            };
            return Err(abandon(engine, launched, cancel, err, e));
        }
    };
    // The first look forwards anything, SIGINT included: what arrived while `claude` was being
    // spawned, the terminal could not deliver to it.
    forward(child.id(), cancel.take(), true);
    let status = wait(&mut child, cancel);
    // What forwarding left is spent, so exit handling meets only a signal sent from now on.
    let _ = cancel.take();
    let code = match status {
        Ok(status) => exit_code(status),
        Err(e) => {
            let _ = writeln!(err, "tagteam: lost track of the session's exit status: {e}");
            LOST
        }
    };
    notify(err, &engine.finish_run(launched, LaunchEnd::Exited(code)));
    Ok(code)
}

/// §12.3, §12.5: a launch refused or interrupted once its reservation exists runs its exit
/// handling as if `claude` had exited at once, then fails with `e`: exit 1, or `128 + n` for an
/// interruption. Whatever signal is pending is spent on that, so the exit handling stops only at
/// a new one (Decision 12): the one that interrupted the launch, or one that arrived after a
/// refusal was decided, which leaves `e` as it is and is never reported as too late.
pub(crate) fn abandon(
    engine: &Engine,
    launched: Launched,
    cancel: &Cancel,
    err: &mut dyn Write,
    e: EngineError,
) -> EngineError {
    let _ = cancel.take();
    notify(err, &engine.finish_run(launched, LaunchEnd::Refused));
    e
}

/// §12.1: `exec` returns only when it failed. A launch command gone since `plan_run` found it
/// is `launch-command-missing`, as if it had never been there; any other failure is the I/O
/// error, naming the command.
pub(crate) fn exec_failed(spec: &SpawnSpec, e: io::Error) -> EngineError {
    if e.kind() == io::ErrorKind::NotFound {
        let command = spec.program.file_name().map_or_else(
            || spec.program.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        return EngineError::LaunchCommandMissing { command };
    }
    EngineError::Io(io::Error::new(
        e.kind(),
        format!("could not run {}: {e}", spec.program.display()),
    ))
}

/// Decision 1, §12.5: whether a signal the token recorded goes on to `claude`. The first look
/// after the spawn forwards SIGINT, SIGTERM and SIGHUP alike. After it, SIGINT is the
/// terminal's to deliver, since `claude` is in its foreground group, so only SIGTERM and
/// SIGHUP, which a `kill` sends to tagteam alone, are forwarded.
pub(crate) fn forwarded(signal: i32, first: bool) -> bool {
    match signal {
        libc::SIGTERM | libc::SIGHUP => true,
        libc::SIGINT => first,
        _ => false,
    }
}

/// Sends `signal`, when `forwarded` says so, to `claude` (`pid`).
fn forward(pid: u32, signal: Option<i32>, first: bool) {
    let Some(signal) = signal.filter(|&s| forwarded(s, first)) else {
        return;
    };
    // SAFETY: kill(2) reads no memory of ours. `pid` is this process's child, which only
    // `wait` reaps, and nothing is sent once it has: the pid names `claude` or its zombie,
    // never a process that reused the number.
    let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if rc != 0 {
        tracing::warn!(
            signal,
            "could not forward the signal to the session: {}",
            io::Error::last_os_error()
        );
    }
}

/// Waits for `claude`, forwarding what the token records meanwhile. Nothing here waits on a
/// lock (B.37).
fn wait(child: &mut Child, cancel: &Cancel) -> io::Result<ExitStatus> {
    let pid = child.id();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            // No status to poll for: wait it out; it is still the child's exit that ends the run.
            Err(_) => return child.wait(),
        }
        thread::sleep(POLL);
        forward(pid, cancel.take(), false);
    }
}

/// Exit handling's notices (Decision 12), on stderr: stdout is `claude`'s.
fn notify(err: &mut dyn Write, notices: &[String]) {
    for notice in notices {
        let _ = writeln!(err, "note: {notice}");
    }
}

/// A test-only stop at `name`: `before-spawn` is right after the token's last look. It parks
/// as the engine's points do (M3a): with `TAGTEAM_TEST_PAUSE_AT=<name>`, it writes `paused` in
/// `TAGTEAM_TEST_PAUSE_DIR` and waits up to 30 s for `resume` there. It never looks at the
/// token, so a signal sent meanwhile meets the run exactly where it would have.
#[cfg(feature = "test-support")]
fn pause_point(name: &str) {
    if std::env::var("TAGTEAM_TEST_PAUSE_AT").as_deref() != Ok(name) {
        return;
    }
    let Some(dir) = std::env::var_os("TAGTEAM_TEST_PAUSE_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    let _ = std::fs::write(dir.join("paused"), b"");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !dir.join("resume").exists() && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(not(feature = "test-support"))]
fn pause_point(_name: &str) {}

#[cfg(test)]
mod tests {
    // `io` and `SpawnSpec` come from the module's own imports.
    use super::*;

    #[test]
    fn the_first_look_after_the_spawn_forwards_any_signal_and_later_ones_only_term_and_hup() {
        // §12.5, Decision 1: (signal, first look) → forwarded to claude.
        let cases = [
            (libc::SIGINT, true, true),
            (libc::SIGTERM, true, true),
            (libc::SIGHUP, true, true),
            (libc::SIGINT, false, false),
            (libc::SIGTERM, false, true),
            (libc::SIGHUP, false, true),
            (libc::SIGQUIT, true, false),
            (libc::SIGQUIT, false, false),
        ];
        for (signal, first, want) in cases {
            assert_eq!(
                forwarded(signal, first),
                want,
                "{signal}, first look {first}"
            );
        }
    }

    #[test]
    fn an_exec_that_finds_no_command_is_launch_command_missing_and_any_other_failure_is_io() {
        let spec = SpawnSpec {
            program: "/opt/bin/claude".into(),
            ..SpawnSpec::default()
        };
        let e = exec_failed(&spec, io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(e.kind(), "launch-command-missing");
        let e = exec_failed(&spec, io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(e.kind(), "io");
        assert!(e.to_string().contains("/opt/bin/claude"), "{e}");
    }
}
