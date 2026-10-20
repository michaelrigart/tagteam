//! §12.5: `tagteam run` once its launch is made. The spawn, the wait, the signals forwarded to
//! `claude`, the exit handling its exit starts, and the exit code (§14.1, B.63). Around them,
//! the token report's evidence (token report spec §6.1): the launch, planned and recorded before
//! the spawn, and the child's session, read while it runs.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use tagteam_engine::launch::{LaunchEnd, Launched};
use tagteam_engine::tokens::{LaunchId, LaunchPoll};
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::process::{SpawnSpec, exit_code, spawn_session};
use tagteam_provider::{Cancel, ExitWatch, Watched};

use crate::signals;

/// How often `run` forwards what the cancel token recorded (Decision 1) and checks the child's
/// session record (H9). The child's exit wakes the wait at once, so a tick never delays it.
const TICK: Duration = Duration::from_millis(250);

/// Token report spec §6.1: how often `run` writes what its checks of the child's session record
/// saw. It checks the record on every `TICK`.
const RECORD_POLL: Duration = Duration::from_secs(2);

/// `run`'s exit code when it lost track of `claude`'s status, which only a failing `waitpid`
/// can cause.
const LOST: i32 = 1;

/// §12.5 steps 6–7 and "When the child exits", after `launch` and the login check. Spawns
/// `launch` with `args` in `cwd` and `launched`'s environment, with the reservation inherited
/// through its fd. Waits for it, forwarding signals (Decision 1), then runs the exit handling
/// and prints its notices on `err`.
///
/// Token report spec §6.1: the launch is planned and recorded before the spawn, or kept in a
/// pending file in its profile, and the child's pid, its session record and a launch record
/// that failed are recorded off this thread while it runs. None of that ever changes the
/// outcome.
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
    let profile = launched.profile.clone();
    let tracked = plan_launch(engine, &launched, &profile, err);
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
            let e = abandon(engine, launched, cancel, err, e);
            let no_sighting = LaunchPoll::default();
            close_launch(
                engine,
                tracked.as_ref(),
                false,
                &profile,
                &no_sighting,
                engine.now_ms(),
                err,
            );
            return Err(e);
        }
    };
    let pid = child.id();
    // The first look forwards anything, SIGINT included: what arrived while `claude` was being
    // spawned, the terminal could not deliver to it.
    forward(pid, cancel.take(), true, &mut kill_child);
    let mut seen = LaunchPoll::default();
    // Token report spec §6.1: the child's exit, stamped as soon as the wait returns, bounds its
    // sighting: a check after it confirms nothing.
    let ((status, _forwarded), exited_at_ms) = match &tracked {
        Some(launch) => wait_polling(engine, launch, &profile, pid, &mut seen, || {
            wait(&mut child, cancel)
        }),
        None => (wait(&mut child, cancel), engine.now_ms()),
    };
    // What forwarding left is spent, so exit handling meets only a signal sent from now on.
    let _ = cancel.take();
    pause_point("after-exit");
    let code = match status {
        Ok(status) => exit_code(status),
        Err(e) => {
            let _ = writeln!(err, "tagteam: lost track of the session's exit status: {e}");
            LOST
        }
    };
    notify(err, &engine.finish_run(launched, LaunchEnd::Exited(code)));
    // After exit handling, which unlinks this process's reservation: until then the profile
    // is in use by this very run, and the launch could never close here.
    close_launch(
        engine,
        tracked.as_ref(),
        true,
        &profile,
        &seen,
        exited_at_ms,
        err,
    );
    Ok(code)
}

/// A launch `run` planned for a provider whose transcripts the report reads (token report spec
/// §6.1), whether it is recorded yet, and whether it is in its profile's pending file. The poller
/// sets the flags; `run` reads them once the poller is joined.
struct TrackedLaunch {
    id: LaunchId,
    recorded: AtomicBool,
    kept: AtomicBool,
}

/// Token report spec §6.1: plans the launch before the spawn, for a provider whose transcripts
/// the report reads, and tries to record it. Its start is stamped here, so a launch recorded
/// later still covers the whole session. One the store refuses is kept in a pending file in the
/// profile, which the reservation just made proves writable, and the session runs all the same:
/// the poller retries the record while the child runs, and `close_launch` tries once more. A
/// launch that neither accepts is one warning on `err`.
fn plan_launch(
    engine: &Engine,
    launched: &Launched,
    profile: &Path,
    err: &mut dyn Write,
) -> Option<TrackedLaunch> {
    let provider = &launched.account.provider;
    if !engine
        .provider(provider)
        .is_ok_and(|p| p.capabilities().transcripts)
    {
        return None;
    }
    let id = engine.plan_launch(provider, &launched.account.id);
    let (recorded, kept) = match engine.record_launch(&id) {
        Ok(()) => (true, false),
        Err(e) => match engine.keep_pending_launch(&id, profile, None) {
            Ok(()) => {
                tracing::debug!(
                    kind = e.kind(),
                    "the store refused the session's launch; it is kept in a pending file"
                );
                (false, true)
            }
            Err(k) => {
                let _ = writeln!(
                    err,
                    "warning: the token report could not record this session's launch, in the store ({e}) or in its profile ({k}). It will try again while the session runs. If it never can, this session's usage may be credited to the default login's account."
                );
                (false, false)
            }
        },
    };
    Some(TrackedLaunch {
        id,
        recorded: AtomicBool::new(recorded),
        kept: AtomicBool::new(kept),
    })
}

/// Token report spec §6.1, after exit handling: records the launch once more if it is still not
/// recorded, ends the child's tenures (`Engine::close_launch`): the current one at the later
/// of its stored end and its last confirming read, the ones a switch ended exactly at theirs, and
/// none past `exited_at_ms`, the child's exit. Then it closes the launch unless the profile is
/// still in use, as by a background session the child left running. A launch still not recorded
/// is kept in its pending file instead, with the close's time when the profile is idle.
///
/// A launch that is in neither the store nor a pending file is logged, and warned of on `err`
/// (the child being gone) when `child_ran`: with no child there is no usage to misplace. A
/// launch in a pending file is logged at debug alone: the next scan records it. The close that
/// follows an unrecorded launch is not warned of again, and the tenures' ends are written all
/// the same. Any other failure to close is a warning, and the next `sight_sessions` closes the
/// launch then.
fn close_launch(
    engine: &Engine,
    tracked: Option<&TrackedLaunch>,
    child_ran: bool,
    profile: &Path,
    seen: &LaunchPoll,
    exited_at_ms: i64,
    err: &mut dyn Write,
) {
    let Some(tracked) = tracked else {
        return;
    };
    let mut recorded = tracked.recorded.load(Ordering::Relaxed);
    let mut refused = None;
    if !recorded {
        match engine.record_launch(&tracked.id) {
            Ok(()) => recorded = true,
            Err(e) => refused = Some(e.kind()),
        }
    }
    let end = engine.close_launch(&tracked.id, profile, seen, exited_at_ms, recorded);
    if let Some(kind) = refused {
        log_unrecorded(kind, end.kept);
    }
    if recorded {
        if let Err(e) = end.stored {
            let _ = writeln!(
                err,
                "warning: the token report could not record this session's end: {e}"
            );
        }
    } else if !end.kept && child_ran {
        let _ = writeln!(
            err,
            "warning: the token report could not record this session's launch, in the store or in its profile, so its usage may be credited to the default login's account"
        );
    }
}

/// Logs a launch the store refused: at debug while its pending file holds it, as a warning only
/// when neither does (the stderr warning's condition).
fn log_unrecorded(kind: &str, kept: bool) {
    if kept {
        tracing::debug!(
            kind,
            "the store refused the session's launch; it is kept in a pending file"
        );
    } else {
        tracing::warn!(
            kind,
            "could not record the session's launch for the token report"
        );
    }
}

/// Token report spec §6.1: runs `wait` on this thread while a scoped thread, until `wait`
/// returns, checks the child's session record on every `TICK`, and every `RECORD_POLL` from
/// its first tick records the launch if it is not yet, records the child's pid once it is, and
/// writes what the checks saw, whether or not the launch is recorded yet. A launch the store
/// still refuses is kept in its pending file meanwhile, written again if that write failed, and
/// the file goes once the launch is recorded. Returns what `wait`
/// returned and the child's exit, stamped by `beside` before the poller is joined. The store
/// writes are the poller's, so a busy store never delays the wait, the signals it forwards or
/// the stamp. The poller never touches the cancel token or the terminal: a failure is logged,
/// once for each kind, and the next tick tries again.
fn wait_polling<T>(
    engine: &Engine,
    launch: &TrackedLaunch,
    profile: &Path,
    pid: u32,
    seen: &mut LaunchPoll,
    wait: impl FnOnce() -> T,
) -> (T, i64) {
    let id = &launch.id;
    let poller = |stop: &AtomicBool| {
        let mut pid_attached = false;
        // The refused record is logged once at each level: debug while a pending file holds
        // the launch, a warning when it does not.
        let mut record_logged = [false; 2];
        let mut next = Instant::now();
        let (mut check_logged, mut write_logged) = (false, false);
        let mut reported = None;
        while !stop.load(Ordering::Relaxed) {
            if let Err(e) = engine.observe_launch(id, profile, pid, seen) {
                if !check_logged {
                    tracing::warn!(
                        kind = e.kind(),
                        "could not read the session's record for the token report"
                    );
                    check_logged = true;
                }
            }
            report_confirmed(seen, &mut reported);
            if Instant::now() >= next {
                if !launch.recorded.load(Ordering::Relaxed) {
                    match engine.record_launch(id) {
                        Ok(()) => launch.recorded.store(true, Ordering::Relaxed),
                        Err(e) => {
                            if !launch.kept.load(Ordering::Relaxed)
                                && engine.keep_pending_launch(id, profile, None).is_ok()
                            {
                                launch.kept.store(true, Ordering::Relaxed);
                            }
                            let kept = launch.kept.load(Ordering::Relaxed);
                            if !record_logged[usize::from(kept)] {
                                log_unrecorded(e.kind(), kept);
                                record_logged[usize::from(kept)] = true;
                            }
                            pause_point("launch-retry-failed");
                        }
                    }
                }
                // Recorded: the pending file is not needed. A removal that fails is tried
                // again on the next write, and by the close.
                if launch.recorded.load(Ordering::Relaxed)
                    && launch.kept.load(Ordering::Relaxed)
                    && engine.drop_pending_launch(id, profile).is_ok()
                {
                    launch.kept.store(false, Ordering::Relaxed);
                }
                if launch.recorded.load(Ordering::Relaxed) && !pid_attached {
                    pid_attached = true;
                    if let Err(e) = engine.attach_launch_pid(id, pid) {
                        tracing::warn!(
                            kind = e.kind(),
                            "could not record the session's pid for the token report"
                        );
                    }
                }
                if let Err(e) = engine.poll_launch(id, seen) {
                    if !write_logged {
                        tracing::warn!(
                            kind = e.kind(),
                            "could not record the session for the token report"
                        );
                        write_logged = true;
                    }
                }
                next = Instant::now() + RECORD_POLL;
            }
            thread::park_timeout(TICK);
        }
    };
    beside(poller, wait, || engine.now_ms())
}

/// Runs `wait` on this thread while a scoped thread runs `poller` until its flag is set, and
/// returns what `wait` returned with `stamp`, read on this thread as soon as `wait` returns:
/// before the flag is set and the poller joined, so a poller blocked in a store write never
/// delays it. The flag is set however `wait` ends, a panic included, and the poller is unparked
/// then, so it does not sleep out the rest of its tick before it sees the flag. A poller that
/// cannot start or that panics loses evidence, never the run (spec §6.1): it is joined here, so
/// the scope has no panic of its own to re-raise, and the failure is one warning in the log.
fn beside<T>(
    poller: impl FnOnce(&AtomicBool) + Send,
    wait: impl FnOnce() -> T,
    stamp: impl FnOnce() -> i64,
) -> (T, i64) {
    let stop = AtomicBool::new(false);
    thread::scope(|s| {
        let poller = thread::Builder::new()
            .name("token-poll".into())
            .spawn_scoped(s, || poller(&stop));
        let poller = match poller {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::warn!("could not start the token report's poller: {e}");
                None
            }
        };
        let guard = StopOnDrop {
            stop: &stop,
            poller: poller.as_ref().map(|handle| handle.thread().clone()),
        };
        let waited = wait();
        let at = stamp();
        drop(guard);
        if poller.is_some_and(|handle| handle.join().is_err()) {
            tracing::warn!("the token report's poller panicked");
        }
        (waited, at)
    })
}

/// Sets its flag when dropped, then unparks the poller, so it ends however the wait does, a
/// panic included, and at once: an unpark before its park is kept, so it cannot sleep through
/// the flag. The scope never waits on it forever.
struct StopOnDrop<'a> {
    stop: &'a AtomicBool,
    poller: Option<thread::Thread>,
}

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(poller) = &self.poller {
            poller.unpark();
        }
    }
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

/// Sends `signal`, when `forwarded` says so, to the child `pid` through `send`. Returns the
/// signal when it was one to forward.
fn forward(
    pid: u32,
    signal: Option<i32>,
    first: bool,
    send: &mut dyn FnMut(u32, i32),
) -> Option<i32> {
    let signal = signal.filter(|&s| forwarded(s, first))?;
    send(pid, signal);
    Some(signal)
}

/// Sends `signal` to the child `pid`, with a warning when it cannot.
fn kill_child(pid: u32, signal: i32) {
    // SAFETY: kill(2) reads no memory of ours. `pid` is this process's child, which only
    // `wait_every` reaps, and nothing is sent once it has: the pid names the child or its
    // zombie, never a process that reused the number.
    let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if rc != 0 {
        tracing::warn!(
            signal,
            "could not forward the signal to the session: {}",
            io::Error::last_os_error()
        );
    }
}

/// Waits for the child, forwarding what the token records meanwhile (Decision 1), and returns
/// its status with the last signal forwarded to it. `run`'s wait for `claude`, and the TUI's for
/// the `tagteam run` it starts (TUI spec §7.3). Nothing here waits on a lock (B.37).
pub(crate) fn wait(child: &mut Child, cancel: &Cancel) -> (io::Result<ExitStatus>, Option<i32>) {
    wait_every(child, cancel, TICK, &mut kill_child)
}

/// `wait` with its tick and its sender given. The child's exit wakes the wait at once; every
/// `tick` until then, and once more after the exit, the token is taken and what it holds goes to
/// `send`. This thread is the only one that reaps, and it sends nothing once it has, so the pid
/// names the child or its zombie, never a process that reused the number. A signal pending as the
/// child exits is thus sent to its zombie and never after the reap. The TUI's `tagteam run` is
/// waited on the same way (TUI spec §7.3). If the exit watch fails there is nothing to tick on:
/// the child is waited out, with no more forwarding.
fn wait_every(
    child: &mut Child,
    cancel: &Cancel,
    tick: Duration,
    send: &mut dyn FnMut(u32, i32),
) -> (io::Result<ExitStatus>, Option<i32>) {
    let pid = child.id();
    let mut exit = ExitWatch::start(pid);
    let mut last = None;
    loop {
        let watched = exit.wait(tick);
        if matches!(watched, Watched::Failed(_)) {
            return (child.wait(), last);
        }
        last = forward(pid, cancel.take(), false, send).or(last);
        if watched == Watched::Exited {
            return (child.wait(), last);
        }
    }
}

/// Exit handling's notices (Decision 12), on stderr: stdout is `claude`'s.
fn notify(err: &mut dyn Write, notices: &[String]) {
    for notice in notices {
        let _ = writeln!(err, "note: {notice}");
    }
}

/// A test-only stop at `name`: `before-spawn` is right after the token's last look,
/// `after-exit` is between `claude`'s exit and its exit handling, while the reservation is still
/// this process's, and `launch-retry-failed` is in the poller, right after a retry of the launch
/// record failed. It parks through the engine's own protocol (`tagteam_engine::pause_at`), and
/// never looks at the token, so a signal sent meanwhile meets the run exactly where it would
/// have.
#[cfg(feature = "test-support")]
fn pause_point(name: &str) {
    tagteam_engine::pause_at(name);
}

#[cfg(not(feature = "test-support"))]
fn pause_point(_name: &str) {}

/// A test-only report of the poller's checks: with `TAGTEAM_TEST_CONFIRMED_AT=<file>`, each time
/// the current tenure's `confirmed_at` moves, the poller writes it to that file (epoch ms), so a
/// binary test can wait for a check after a store write rather than sleep. `reported` is what it
/// wrote last.
#[cfg(feature = "test-support")]
fn report_confirmed(seen: &LaunchPoll, reported: &mut Option<i64>) {
    let Some(path) = std::env::var_os("TAGTEAM_TEST_CONFIRMED_AT") else {
        return;
    };
    let Some(at) = seen.tenure.as_ref().map(|t| t.confirmed_at) else {
        return;
    };
    if *reported != Some(at) {
        *reported = Some(at);
        let _ = std::fs::write(path, at.to_string());
    }
}

#[cfg(not(feature = "test-support"))]
fn report_confirmed(_seen: &LaunchPoll, _reported: &mut Option<i64>) {}

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

    #[test]
    fn the_exit_is_stamped_before_the_poller_is_joined() {
        // Token report spec §6.1: a poller stuck in a store write. It goes on only once the
        // stamp is read, so a stamp read after the join would find it finished; the 5 s bound
        // only keeps such a failure from hanging.
        let (stamped, finished) = (AtomicBool::new(false), AtomicBool::new(false));
        let poller = |_stop: &AtomicBool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !stamped.load(Ordering::SeqCst) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            finished.store(true, Ordering::SeqCst);
        };
        let stamp = || {
            let busy = !finished.load(Ordering::SeqCst);
            stamped.store(true, Ordering::SeqCst);
            i64::from(busy)
        };
        assert_eq!(
            beside(poller, || "exited", stamp),
            ("exited", 1),
            "the stamp is read while the poller is still busy"
        );
        assert!(finished.load(Ordering::SeqCst), "and the poller is joined");
    }

    #[test]
    fn the_stop_flag_is_set_however_the_wait_ends() {
        // A poller that runs until the flag is set (5 s at most, so a regression fails rather
        // than hangs) and a wait that panics: `beside` must unwind, with the poller stopped by
        // the flag.
        let stopped_by_flag = AtomicBool::new(false);
        let poller = |stop: &AtomicBool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            stopped_by_flag.store(stop.load(Ordering::Relaxed), Ordering::SeqCst);
        };
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            beside(poller, || -> i32 { panic!("the wait failed") }, || 0)
        }));
        assert!(unwound.is_err());
        assert!(
            stopped_by_flag.load(Ordering::SeqCst),
            "a panicking wait still stops the poller"
        );
    }

    #[test]
    fn a_panicking_poller_loses_evidence_and_never_the_run() {
        // Spec §6.1: the wait's result and the stamp come back as if the poller had finished.
        let poller = |_stop: &AtomicBool| panic!("the poll failed");
        assert_eq!(beside(poller, || 7, || 1), (7, 1));
    }

    #[test]
    fn a_poller_that_ends_by_itself_is_no_trouble() {
        assert_eq!(
            beside(|_stop: &AtomicBool| {}, || "exited", || 3),
            ("exited", 3)
        );
    }

    /// `pid`, a child of this process, is not reaped yet: waitid answers for it (running or a
    /// zombie), and fails with ECHILD once it is reaped.
    fn unreaped(pid: u32) -> bool {
        // SAFETY: a zeroed siginfo_t that waitid fills in; WNOHANG|WNOWAIT reap nothing.
        unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            libc::waitid(
                libc::P_PID,
                pid,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            ) == 0
        }
    }

    /// `f`'s result from a thread of its own; a panic once `limit` has passed, so a wait that only
    /// a tick would end fails instead of hanging.
    fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(limit).expect("it did not return in time")
    }

    #[test]
    fn a_signal_pending_as_the_child_exits_is_forwarded_while_it_is_unreaped_never_after() {
        // Review Focus 3, H9: the child has exited, unreaped, when the wait first reads the token.
        let cancel = Cancel::new();
        let token = cancel.clone();
        let ((status, last), sends, pid) = within(Duration::from_secs(10), move || {
            let mut child = std::process::Command::new("/bin/sh")
                .args(["-c", "exit 3"])
                .spawn()
                .unwrap();
            let pid = child.id();
            assert_eq!(
                ExitWatch::start(pid).wait(Duration::from_secs(5)),
                Watched::Exited
            );
            token.request(libc::SIGTERM);
            let mut sends = Vec::new();
            let waited = wait_every(
                &mut child,
                &token,
                Duration::from_secs(3600),
                &mut |p, s| sends.push((s, unreaped(p))),
            );
            (waited, sends, pid)
        });
        assert_eq!(status.unwrap().code(), Some(3));
        assert_eq!(last, Some(libc::SIGTERM));
        assert_eq!(
            sends,
            [(libc::SIGTERM, true)],
            "sent to the zombie, before the reap"
        );
        assert!(
            !unreaped(pid),
            "reaped before the wait returned, so nothing can follow"
        );
        assert_eq!(cancel.requested(), None, "taken once");
    }

    #[test]
    fn a_signal_while_the_child_runs_reaches_it_within_a_tick_and_only_while_unreaped() {
        let d = tempfile::tempdir().unwrap();
        let started = d.path().join("started");
        let script = format!(
            "trap 'exit 7' TERM; : > '{}'; i=0; while [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done",
            started.display()
        );
        let cancel = Cancel::new();
        let (token, signaller) = (cancel.clone(), cancel.clone());
        let sender = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !started.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            signaller.request(libc::SIGTERM);
        });
        let ((status, last), sends) = within(Duration::from_secs(10), move || {
            let mut child = std::process::Command::new("/bin/sh")
                .args(["-c", &script])
                .spawn()
                .unwrap();
            let mut sends = Vec::new();
            let waited = wait_every(
                &mut child,
                &token,
                Duration::from_millis(20),
                &mut |p, s| {
                    sends.push((s, unreaped(p)));
                    kill_child(p, s); // deliver it, as `wait` does: the trap's exit 7 is the proof
                },
            );
            (waited, sends)
        });
        sender.join().unwrap();
        assert_eq!(
            status.unwrap().code(),
            Some(7),
            "the forwarded SIGTERM ended it"
        );
        assert_eq!(last, Some(libc::SIGTERM));
        assert_eq!(sends, [(libc::SIGTERM, true)]);
    }

    #[test]
    fn the_child_s_exit_is_reported_without_waiting_for_a_tick() {
        // H14: an hour's tick; the exit wakes the wait.
        let ((status, last), sends) = within(Duration::from_secs(10), || {
            let mut child = std::process::Command::new("/bin/sh")
                .args(["-c", "sleep 0.1; exit 5"])
                .spawn()
                .unwrap();
            let mut sends = 0;
            let waited = wait_every(
                &mut child,
                &Cancel::new(),
                Duration::from_secs(3600),
                &mut |_, _| sends += 1,
            );
            (waited, sends)
        });
        assert_eq!(status.unwrap().code(), Some(5));
        assert_eq!((last, sends), (None, 0));
    }

    #[test]
    fn the_poller_is_woken_as_soon_as_the_wait_returns() {
        // H9: the poller sleeps a 250 ms tick, and `run` never waits it out at exit.
        let poller = |stop: &AtomicBool| {
            while !stop.load(Ordering::Relaxed) {
                thread::park_timeout(Duration::from_secs(20));
            }
        };
        let started = Instant::now();
        assert_eq!(beside(poller, || "exited", || 1), ("exited", 1));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }
}
