//! The terminal, descriptor and job-control calls that the development harness (`xtask`) and
//! the TUI (`tagteam-tui`) need, as safe functions: the workspace allows `unsafe` only here and
//! in `tagteam`, so both forbid it and call these.

use std::io;
use std::os::fd::RawFd;
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command};

/// How a wait for a descriptor to become readable ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Input is waiting, or the peer closed (a read then sees the end).
    Ready,
    /// The timeout passed with nothing to read.
    TimedOut,
    /// A signal ended the wait early; ask again.
    Interrupted,
    /// The descriptor is not open.
    Invalid,
    /// `poll(2)` failed some other way.
    Failed,
}

/// Waits up to `timeout_ms` for `fd` to be readable (`poll(2)` on one descriptor).
pub fn wait_readable(fd: RawFd, timeout_ms: i32) -> Readiness {
    let mut fds = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `fds` is one valid, writable `pollfd` for the duration of the call.
    let ready = unsafe { libc::poll(&mut fds, 1, timeout_ms) };
    if ready < 0 {
        return if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            Readiness::Interrupted
        } else {
            Readiness::Failed
        };
    }
    if ready == 0 {
        Readiness::TimedOut
    } else if fds.revents & libc::POLLNVAL != 0 {
        Readiness::Invalid
    } else {
        Readiness::Ready
    }
}

/// What reading one byte gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRead {
    Byte(u8),
    /// The end of input.
    End,
    /// A signal ended the read early; ask again.
    Interrupted,
    Failed,
}

/// Reads exactly one byte from `fd` (`read(2)`), taking nothing past it.
pub fn read_byte(fd: RawFd) -> ByteRead {
    let mut byte = 0u8;
    // SAFETY: `byte` is one valid, writable byte for the duration of the call.
    let n = unsafe { libc::read(fd, (&raw mut byte).cast(), 1) };
    match n {
        1 => ByteRead::Byte(byte),
        0 => ByteRead::End,
        _ if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted => {
            ByteRead::Interrupted
        }
        _ => ByteRead::Failed,
    }
}

/// This process's group id.
pub fn own_process_group() -> i32 {
    // SAFETY: getpgrp has no arguments and cannot fail.
    unsafe { libc::getpgrp() }
}

/// The group of process `pid`, if it can be told.
pub fn process_group_of(pid: u32) -> Option<u32> {
    let pid = libc::pid_t::try_from(pid).ok()?;
    // SAFETY: getpgid takes a process id and returns a group id; it touches no memory.
    let group = unsafe { libc::getpgid(pid) };
    u32::try_from(group).ok()
}

/// Whether stdin is a terminal whose foreground group is this process's own, so the terminal
/// may be handed to a child's group and taken back.
pub fn stdin_is_our_foreground_terminal() -> bool {
    // SAFETY: isatty and tcgetpgrp only read descriptor 0's state; getpgrp has no arguments.
    unsafe { libc::isatty(0) == 1 && libc::tcgetpgrp(0) == libc::getpgrp() }
}

/// Makes `pgid` the foreground group of the terminal on stdin (`tcsetpgrp`), with SIGTTOU
/// ignored around the call: a background group calling it would be stopped otherwise.
pub fn set_foreground_group(pgid: i32) {
    // SAFETY: SIGTTOU's disposition is set to ignore and put back; `tcsetpgrp` takes a group id
    // and no pointer. Nothing else in the harness handles SIGTTOU.
    unsafe {
        let before = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        libc::tcsetpgrp(0, pgid);
        libc::signal(libc::SIGTTOU, before);
    }
}

/// The foreground group of the terminal on stdin, if stdin is a terminal.
pub fn stdin_foreground_group() -> Option<i32> {
    // SAFETY: isatty and tcgetpgrp only read descriptor 0's state.
    unsafe {
        if libc::isatty(0) != 1 {
            return None;
        }
        let group = libc::tcgetpgrp(0);
        (group > 0).then_some(group)
    }
}

/// Starts `cmd` as a job of its own, as a shell starts a foreground job: in a process group of
/// its own and, when stdin is a terminal held by this process's group, as that terminal's
/// foreground group before it execs, so it never reads the terminal from the background and is
/// stopped by SIGTTIN. The job's SIGTTOU, SIGTTIN, SIGTSTP, SIGINT and SIGQUIT are the default
/// ones. The caller still hands the terminal over itself (the classic double handoff, closing the
/// race either way) and takes it back.
pub fn spawn_foreground(cmd: &mut Command) -> io::Result<Child> {
    let take_terminal = stdin_is_our_foreground_terminal();
    // SAFETY: the closure runs in the forked child before exec and calls only
    // async-signal-safe functions (setpgid, signal, tcsetpgrp, getpid), touching no memory of
    // the parent's but its own stack.
    unsafe {
        cmd.pre_exec(move || {
            libc::setpgid(0, 0);
            if take_terminal {
                libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                libc::tcsetpgrp(0, libc::getpid());
            }
            for sig in [
                libc::SIGTTOU,
                libc::SIGTTIN,
                libc::SIGTSTP,
                libc::SIGINT,
                libc::SIGQUIT,
            ] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    let spawned = cmd.spawn();
    if spawned.is_err() && take_terminal {
        // The child took the terminal before its exec failed (a missing program or
        // interpreter): it is gone, so the terminal goes back to this process's group, or the
        // next read of it from the background would stop the harness with SIGTTIN.
        set_foreground_group(own_process_group());
    }
    spawned
}

/// Whether process `pid`, a child of this process, is stopped (SIGSTOP, SIGTSTP, SIGTTIN or
/// SIGTTOU), without reaping it or consuming any other report: `waitid` with `WNOWAIT`.
pub fn child_is_stopped(pid: u32) -> bool {
    let pid: libc::id_t = pid;
    // SAFETY: `info` is a zeroed `siginfo_t` that `waitid` fills in; WNOHANG and WNOWAIT make it
    // return at once and leave the child waitable.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        let rc = libc::waitid(
            libc::P_PID,
            pid,
            &mut info,
            libc::WSTOPPED | libc::WNOHANG | libc::WNOWAIT,
        );
        rc == 0 && info.si_signo == libc::SIGCHLD && info.si_code == libc::CLD_STOPPED
    }
}

/// Ctrl-Z for a program that reads its keys raw (TUI spec §10.1): stops this process's group
/// with SIGTSTP, as the terminal would have, and returns once it is continued (SIGCONT). The
/// group, not the process alone, as vim and less do: under a wrapper (`cargo run`, a shim) the
/// shell's job is the wrapper's group, and it sees the job stop only when all of it has.
/// Where nothing does job control the signal is discarded, and this returns at once.
pub fn stop_self() -> io::Result<()> {
    // SAFETY: kill(2) with pid 0 takes a signal number and signals the caller's own process
    // group; it touches no memory of ours.
    if unsafe { libc::kill(0, libc::SIGTSTP) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::net::UnixStream;

    use super::*;

    #[test]
    fn a_descriptor_is_ready_once_it_has_a_byte_and_read_gives_one_byte_at_a_time() {
        let (r, mut w) = UnixStream::pair().unwrap();
        let fd = r.as_raw_fd();
        assert_eq!(wait_readable(fd, 10), Readiness::TimedOut);
        w.write_all(b"ab").unwrap();
        assert_eq!(wait_readable(fd, 1000), Readiness::Ready);
        assert_eq!(read_byte(fd), ByteRead::Byte(b'a'));
        assert_eq!(read_byte(fd), ByteRead::Byte(b'b'));
        drop(w);
        assert_eq!(
            wait_readable(fd, 1000),
            Readiness::Ready,
            "a closed peer is readable"
        );
        assert_eq!(read_byte(fd), ByteRead::End);
    }

    #[test]
    fn a_descriptor_that_is_not_open_is_invalid() {
        assert_eq!(wait_readable(1_000_000, 10), Readiness::Invalid);
        assert_eq!(read_byte(1_000_000), ByteRead::Failed);
    }

    #[test]
    fn the_process_group_calls_agree_about_this_process() {
        let me = std::process::id();
        assert_eq!(
            process_group_of(me),
            u32::try_from(own_process_group()).ok()
        );
        assert_eq!(process_group_of(u32::MAX), None);
    }
}

#[cfg(test)]
mod spawn_tests {
    use super::*;

    #[test]
    fn a_program_that_cannot_start_is_an_error_and_changes_nothing_without_a_terminal() {
        // The tests' stdin is no terminal, so no terminal was taken and none is given back; the
        // path with one is exercised only by a live run. `spawn_foreground` forks before the exec
        // fails, so the fork guard is held.
        let _fork = crate::fork_guard();
        let before = stdin_foreground_group();
        let err = spawn_foreground(&mut Command::new("/nonexistent/program")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(stdin_foreground_group(), before);
    }

    #[test]
    fn stop_self_stops_the_process_until_it_is_continued() {
        // A forked child stops itself; the test sees it stopped, continues it, and sees it exit
        // 0. The child leads a group of its own first: in an orphaned group, as a CI runner's
        // may be, SIGTSTP is discarded.
        let _fork = crate::fork_guard();
        // SAFETY: the child calls only async-signal-safe functions: setpgid(2), raise(3)
        // inside `stop_self`, and _exit(2).
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork: {}", io::Error::last_os_error());
        if pid == 0 {
            // SAFETY: in the forked child; both take integers and touch no memory.
            unsafe {
                libc::setpgid(0, 0);
                libc::_exit(if stop_self().is_ok() { 0 } else { 1 });
            }
        }
        let mut status = 0;
        // SAFETY: waitpid(2) writes the child's status into `status`; WUNTRACED reports a stop.
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) },
            pid
        );
        assert!(
            libc::WIFSTOPPED(status) && libc::WSTOPSIG(status) == libc::SIGTSTP,
            "{status:#x}"
        );
        // SAFETY: kill(2) on this test's own child, which is not reaped yet.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGCONT) }, 0);
        // SAFETY: as above, now for its exit.
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "{status:#x}"
        );
    }

    #[test]
    fn stop_self_stops_every_process_of_the_group() {
        // A forked child leads a group of its own, and a second child joins it. When the leader
        // stops itself the other stops too, as a wrapper's shell would need.
        let _fork = crate::fork_guard();
        let mut ready = [0; 2];
        // SAFETY: pipe(2) fills the two descriptors of `ready`.
        assert_eq!(unsafe { libc::pipe(ready.as_mut_ptr()) }, 0);
        // SAFETY: the children call only async-signal-safe functions: read(2), pause(2),
        // `stop_self`'s kill(2), and _exit(2).
        let leader = unsafe { libc::fork() };
        assert!(leader >= 0, "fork: {}", io::Error::last_os_error());
        if leader == 0 {
            let mut byte = 0u8;
            // SAFETY: in the forked child; read(2) writes one byte into `byte`.
            unsafe {
                libc::read(ready[0], (&raw mut byte).cast(), 1);
                libc::_exit(if stop_self().is_ok() { 0 } else { 1 });
            }
        }
        // SAFETY: as above; the second child does nothing but wait for a signal.
        let member = unsafe { libc::fork() };
        assert!(member >= 0, "fork: {}", io::Error::last_os_error());
        if member == 0 {
            // SAFETY: in the forked child; pause(2) takes nothing.
            unsafe {
                loop {
                    libc::pause();
                }
            }
        }
        // SAFETY: setpgid(2) on this test's own children, neither reaped; write(2) reads one
        // byte of `byte`.
        unsafe {
            assert_eq!(libc::setpgid(leader, leader), 0);
            assert_eq!(libc::setpgid(member, leader), 0);
            let byte = 0u8;
            assert_eq!(libc::write(ready[1], (&raw const byte).cast(), 1), 1);
        }
        let stopped = |pid: libc::pid_t| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let mut status = 0;
                // SAFETY: waitpid(2) writes the child's status into `status`; WUNTRACED reports
                // a stop and WNOHANG returns at once.
                let reaped =
                    unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED | libc::WNOHANG) };
                if reaped == pid {
                    return libc::WIFSTOPPED(status) && libc::WSTOPSIG(status) == libc::SIGTSTP;
                }
                if std::time::Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        let (leader_stopped, member_stopped) = (stopped(leader), stopped(member));
        // SAFETY: kill(2) and waitpid(2) on this test's own children, which are not reaped yet;
        // close(2) on the pipe's descriptors.
        unsafe {
            for pid in [leader, member] {
                libc::kill(pid, libc::SIGKILL);
                libc::waitpid(pid, std::ptr::null_mut(), 0);
            }
            libc::close(ready[0]);
            libc::close(ready[1]);
        }
        assert!(leader_stopped, "the leader stopped");
        assert!(member_stopped, "the rest of the group stopped with it");
    }
}
