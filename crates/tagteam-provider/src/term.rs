//! The terminal and descriptor calls the development harness (`xtask`) needs, as safe
//! functions: the workspace allows `unsafe` only here and in `tagteam`, so `xtask` forbids it
//! and calls these.

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
    cmd.spawn()
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
