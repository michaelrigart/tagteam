use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use tagteam_provider::Cancel;

pub trait Prompter {
    /// True only when a person can answer: stdin and stderr are both terminals.
    fn interactive(&self) -> bool;
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool;
    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize>;
    fn secret(&mut self, question: &str) -> Option<String>;
}

/// The controlling terminal's generic name: the last resort when neither stdin nor stderr names
/// a device (`terminal_path`).
const TERMINAL: &str = "/dev/tty";

/// The terminal's prompts. Each waits for its answer in slices the cancel token can end, and
/// never blocks in a read (Decision 5): a prompt a signal cuts short answers as a decline
/// (`false` or `None`), and the CLI, which checks the token after every prompt, reports the
/// interruption.
pub struct TtyPrompter {
    cancel: Cancel,
}

impl TtyPrompter {
    pub fn new(cancel: Cancel) -> Self {
        Self { cancel }
    }

    /// Asks `text` on stderr, then reads one answer from the terminal (`read_line`). `None`
    /// once the token is set: before asking, if it already was, or while waiting; then the
    /// prompt's line is ended, so what follows does not share it with the `^C` the terminal
    /// echoed.
    fn answer(&self, text: &str) -> Option<String> {
        if self.cancel.requested().is_some() {
            return None;
        }
        ask(text);
        let read = open_prompt_terminal().and_then(|tty| read_line(&tty, &self.cancel));
        // The terminal echoed no newline when the prompt was cut short or could not be read.
        if matches!(read, Ok(LineRead::Interrupted) | Err(_)) {
            ask("\n");
        }
        answer_of(read)
    }
}

/// The terminal at `path` opened afresh for one prompt: read and write, non-blocking, and never
/// as a controlling terminal. A fresh open is a file description of tagteam's own, so
/// `O_NONBLOCK` never reaches fd 0's, which the shell shares (a `dup` would share it too), and
/// nothing needs putting back: it closes with the prompt. Whether to prompt at all is still
/// `interactive`'s call, from stdin and stderr; the person answers on that terminal
/// (`terminal_path`).
fn open_terminal(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
}

/// `open_terminal` on `path`, else on `fallback`: the device a stream names may not be ours to
/// open (after `su` in the same pty it is `crw--w----`, EACCES), where `/dev/tty` still is.
/// The first error is the one reported.
fn open_terminal_or(path: &Path, fallback: &Path) -> io::Result<File> {
    open_terminal(path).or_else(|e| open_terminal(fallback).map_err(|_| e))
}

/// The terminal a prompt opens: stdin's or stderr's device by name, else `/dev/tty`
/// (`open_terminal_or`).
fn open_prompt_terminal() -> io::Result<File> {
    open_terminal_or(&terminal_path(), Path::new(TERMINAL))
}

/// The terminal device `fd` is, by its name (`/dev/ttys003`, `/dev/pts/3`), if it is one.
/// `ttyname_r`, never `ttyname`: its buffer is the caller's, so it is safe from any thread.
fn device_of(fd: BorrowedFd<'_>) -> Option<PathBuf> {
    let mut name = [0 as libc::c_char; 1024];
    // SAFETY: `name` is writable for the length passed, and `fd` is open for the call.
    let rc = unsafe { libc::ttyname_r(fd.as_raw_fd(), name.as_mut_ptr(), name.len()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: ttyname_r returned 0, so `name` holds a NUL-terminated path.
    let name = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) };
    Some(PathBuf::from(OsStr::from_bytes(name.to_bytes())))
}

/// The terminal a prompt reads and writes: the first of `fds` that is a terminal, by its own
/// device name, else `/dev/tty`. macOS's `poll(2)` reports `POLLNVAL` at once on `/dev/tty`, so
/// a wait on it would never sleep; the device it stands for polls normally. `interactive` has
/// already required stdin and stderr to be terminals, and with no controlling terminal they
/// still name the device, so the line prompts keep working there.
fn terminal_among(fds: &[BorrowedFd<'_>]) -> PathBuf {
    fds.iter()
        .find_map(|fd| device_of(*fd))
        .unwrap_or_else(|| PathBuf::from(TERMINAL))
}

/// The terminal for a prompt: stdin's device, else stderr's (`terminal_among`).
fn terminal_path() -> PathBuf {
    terminal_among(&[io::stdin().as_fd(), io::stderr().as_fd()])
}

/// How long one `poll(2)` waits before the token is looked at again (Decision 5).
const SLICE_MS: libc::c_int = 100;

/// Waits until `input` (an open descriptor, or a number poll reports as `POLLNVAL`) has
/// something to read, or until `cancel` holds a signal: `true` to go on and read, `false` when
/// interrupted. The handler restarts interrupted syscalls (Decision 2), so a blocked read would
/// never notice the signal; `poll(2)` in 100 ms slices, with the token looked at between them,
/// does. Readiness is only a hint, and the read that follows is non-blocking (`read_line_between`).
/// `POLLIN` is ready at once (a closed peer reports it too, so the read sees the end). A poll
/// that returns at once for any other reason (`POLLNVAL`, a hang-up or an error with nothing to
/// read, a failure other than EINTR) sleeps out its slice first, so the loop never spins, and
/// then reports ready as well: the read decides. `WouldBlock` goes round again, one read per
/// slice; end of input and a read error decline.
fn wait_for_input(input: RawFd, cancel: &Cancel) -> bool {
    loop {
        if cancel.requested().is_some() {
            return false;
        }
        let mut fds = libc::pollfd {
            fd: input,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `fds` is one valid, writable `pollfd` for the duration of the call; poll(2)
        // takes any descriptor number and reports one that is not open as POLLNVAL.
        let ready = unsafe { libc::poll(&mut fds, 1, SLICE_MS) };
        if ready > 0 && fds.revents & libc::POLLIN != 0 {
            return true;
        }
        let returned_early = ready > 0
            || (ready < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted);
        if returned_early {
            std::thread::sleep(std::time::Duration::from_millis(SLICE_MS as u64));
            return cancel.requested().is_none();
        }
    }
}

/// Discards what was typed but not yet read, so an interrupted prompt's half answer never
/// reaches the shell once tagteam exits.
fn discard_input(tty: BorrowedFd<'_>) {
    // SAFETY: tcflush(3) only acts on the terminal behind `tty`, which is open for the call.
    let _ = unsafe { libc::tcflush(tty.as_raw_fd(), libc::TCIFLUSH) };
}

/// How a prompt's read ended. No derived `Debug`: the line may be a secret.
#[derive(PartialEq, Eq)]
enum LineRead {
    /// The line typed, without its line ending.
    Line(String),
    /// End of input (Ctrl-D) before anything was typed.
    End,
    /// The cancel token was set before the line was complete; what was typed is discarded.
    Interrupted,
}

impl std::fmt::Debug for LineRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LineRead::Line(s) => write!(f, "Line(<{} bytes>)", s.len()),
            LineRead::End => f.write_str("End"),
            LineRead::Interrupted => f.write_str("Interrupted"),
        }
    }
}

/// Reads one line from `tty`, which `open_terminal` opened non-blocking (§14.1, Decision 5).
fn read_line(tty: &File, cancel: &Cancel) -> io::Result<LineRead> {
    read_line_between(tty, cancel, || {})
}

/// `read_line`, running `between` each time the wait reports input, just before the read: the
/// window in which a Ctrl-C at the terminal can flush the line the wait saw (the tests use it).
/// Readiness is only a hint. The read is non-blocking, so a line flushed in that window makes
/// it report `WouldBlock`, never wait, and the loop goes back to the token. What each read
/// returns is kept until a newline or the end of input. Interrupted, it discards what was
/// typed, so a half answer never reaches the shell once tagteam exits.
fn read_line_between(
    tty: &File,
    cancel: &Cancel,
    mut between: impl FnMut(),
) -> io::Result<LineRead> {
    let (mut line, mut chunk, mut reader) = (Vec::new(), [0u8; 256], tty);
    loop {
        if !wait_for_input(tty.as_raw_fd(), cancel) {
            discard_input(tty.as_fd());
            return Ok(LineRead::Interrupted);
        }
        between();
        match reader.read(&mut chunk) {
            Ok(0) if line.is_empty() => return Ok(LineRead::End),
            Ok(0) => return line_of(line),
            Ok(n) => {
                line.extend_from_slice(&chunk[..n]);
                if line.contains(&b'\n') {
                    return line_of(line);
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    }
}

/// The line as read, without its line ending. A line that is not UTF-8 is an error that never
/// repeats the bytes.
fn line_of(mut line: Vec<u8>) -> io::Result<LineRead> {
    if let Some(end) = line.iter().position(|&b| b == b'\n') {
        line.truncate(end);
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map(LineRead::Line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the line is not UTF-8"))
}

/// A line prompt's answer, trimmed. `None` at end of input (Ctrl-D), on an interruption or on
/// a read error: every prompt takes that as a decline, never as the default answer an empty
/// line (Enter) gives.
fn answer_of(read: io::Result<LineRead>) -> Option<String> {
    match read {
        Ok(LineRead::Line(s)) => Some(s.trim().to_owned()),
        Ok(LineRead::End | LineRead::Interrupted) | Err(_) => None,
    }
}

/// One line from the terminal stdin is, for `add-token -`: read as a prompt reads one, in slices
/// the token ends, so Ctrl-C does not wait for Enter (the handler restarts a plain read). `None`
/// once a signal ended it; end of input reads as an empty line, as a plain read would.
pub fn read_terminal_line(cancel: &Cancel) -> io::Result<Option<String>> {
    let tty = open_prompt_terminal()?;
    match read_line(&tty, cancel)? {
        LineRead::Line(line) => Ok(Some(line)),
        LineRead::End => Ok(Some(String::new())),
        LineRead::Interrupted => {
            // The terminal echoed no newline: end the line the `^C` is on.
            ask("\n");
            Ok(None)
        }
    }
}

/// The terminal's settings.
fn termios(tty: BorrowedFd<'_>) -> io::Result<libc::termios> {
    let mut t = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: `t` is writable for one `termios`, and `tty` is open for the call.
    if unsafe { libc::tcgetattr(tty.as_raw_fd(), t.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: tcgetattr returned 0, so it filled `t` in.
    Ok(unsafe { t.assume_init() })
}

fn set_termios(tty: BorrowedFd<'_>, t: &libc::termios) -> io::Result<()> {
    // SAFETY: `t` is a whole `termios` read from a terminal, and `tty` is open for the call.
    if unsafe { libc::tcsetattr(tty.as_raw_fd(), libc::TCSANOW, t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The terminal with its echo off, put back exactly as it was when this is dropped: on every
/// return, and while a panic unwinds (§14.1: an interrupted prompt restores the terminal).
struct EchoOff<'a> {
    tty: BorrowedFd<'a>,
    saved: libc::termios,
}

impl<'a> EchoOff<'a> {
    /// Turns `ECHO` and `ECHONL` off, and nothing else. `ICANON` stays on, so the terminal
    /// edits the line (backspace, Ctrl-U) and hands it over whole. `ISIG` stays on, so a
    /// Ctrl-C raises SIGINT, which the handler turns into the token, and is never a byte of the
    /// secret.
    fn new(tty: BorrowedFd<'a>) -> io::Result<Self> {
        let saved = termios(tty)?;
        let mut quiet = saved;
        quiet.c_lflag &= !(libc::ECHO | libc::ECHONL);
        set_termios(tty, &quiet)?;
        Ok(Self { tty, saved })
    }
}

impl Drop for EchoOff<'_> {
    fn drop(&mut self) {
        let _ = set_termios(self.tty, &self.saved);
    }
}

/// Reads one line from the terminal `tty` with its echo off (`EchoOff`), as `read_line` does,
/// so a signal ends a secret prompt as promptly as any other (§14.1). The terminal's settings
/// are back as they were whichever way it returns. A line must fit the terminal's line limit
/// (1024 bytes on macOS), far above any token; `add-token -` takes a longer one from a pipe.
fn read_secret(tty: &File, cancel: &Cancel) -> io::Result<LineRead> {
    read_secret_between(tty, cancel, || {})
}

/// `read_secret`, with `read_line_between`'s window.
fn read_secret_between(tty: &File, cancel: &Cancel, between: impl FnMut()) -> io::Result<LineRead> {
    let _echo_off = EchoOff::new(tty.as_fd())?;
    read_line_between(tty, cancel, between)
}

/// Writes a prompt to stderr, so stdout stays the command's output. A failed write is
/// ignored rather than panicking: the answer read next decides either way.
fn ask(text: &str) {
    let mut err = std::io::stderr().lock();
    let _ = write!(err, "{text}");
    let _ = err.flush();
}

fn confirmed(answer: Option<String>, default_yes: bool) -> bool {
    match answer.map(|a| a.to_ascii_lowercase()).as_deref() {
        None => false,
        Some("") => default_yes,
        Some(a) => a == "y" || a == "yes",
    }
}

/// The 0-based index of a 1-based answer within `count` options.
fn chosen(answer: Option<String>, count: usize) -> Option<usize> {
    answer?
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=count).contains(n))
        .map(|n| n - 1)
}

impl Prompter for TtyPrompter {
    fn interactive(&self) -> bool {
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
    }

    fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
        confirmed(self.answer(&format!("{question} {hint} ")), default_yes)
    }

    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize> {
        let mut text: String = options
            .iter()
            .enumerate()
            .map(|(i, o)| format!("  {}) {o}\n", i + 1))
            .collect();
        text.push_str(&format!("{question} [1-{}] ", options.len()));
        chosen(self.answer(&text), options.len())
    }

    /// The no-echo prompt (§10.2), on the terminal itself as before: the question is written
    /// there and the line read from it by `read_secret`, which a signal ends as promptly as any
    /// other prompt and which leaves the terminal as it found it. The Enter typed was not
    /// echoed, so the line is ended here. A terminal that cannot be opened or set up declines.
    fn secret(&mut self, question: &str) -> Option<String> {
        if self.cancel.requested().is_some() {
            return None;
        }
        let tty = open_prompt_terminal().ok()?;
        let _ = (&tty).write_all(question.as_bytes());
        let read = read_secret(&tty, &self.cancel);
        let _ = (&tty).write_all(b"\n");
        match read {
            Ok(LineRead::Line(secret)) => Some(secret),
            Ok(LineRead::End | LineRead::Interrupted) | Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, OsStr};
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    /// A line prompt's answer to `typed`.
    fn line(typed: &str) -> Option<String> {
        answer_of(Ok(LineRead::Line(typed.to_owned())))
    }

    #[test]
    fn ctrl_d_at_a_yes_by_default_prompt_declines() {
        // Ctrl-D on an empty line: end of input, with nothing read.
        assert_eq!(answer_of(Ok(LineRead::End)), None);
        assert!(!confirmed(answer_of(Ok(LineRead::End)), true));
        // Enter is an empty line, and takes the default.
        assert!(confirmed(line(""), true));
        assert!(!confirmed(line(""), false));
        assert!(confirmed(line(" Yes"), false));
        assert!(!confirmed(line("n"), true));
    }

    #[test]
    fn a_read_error_or_an_interruption_declines() {
        let broken = answer_of(Err(io::Error::other("the terminal went away")));
        assert_eq!(broken, None);
        assert!(!confirmed(broken, true));
        assert!(!confirmed(answer_of(Ok(LineRead::Interrupted)), true));
    }

    #[test]
    fn a_choice_is_one_based_and_in_range() {
        assert_eq!(chosen(line("2"), 2), Some(1));
        assert_eq!(chosen(line("3"), 2), None);
        assert_eq!(chosen(line("0"), 2), None);
        assert_eq!(chosen(answer_of(Ok(LineRead::End)), 2), None);
    }

    /// Sets `cancel` to `signal` from another thread after `after`, as the handler would.
    fn set_after(cancel: &Cancel, after: Duration, signal: i32) -> thread::JoinHandle<()> {
        let cancel = cancel.clone();
        thread::spawn(move || {
            thread::sleep(after);
            cancel.request(signal);
        })
    }

    #[test]
    fn a_signal_ends_a_wait_for_input_that_never_comes_within_a_slice() {
        // Decision 5: the handler restarts syscalls, so only the slices end the wait.
        let (quiet, _writer) = UnixStream::pair().unwrap();
        let cancel = Cancel::new();
        let started = Instant::now();
        let setter = set_after(&cancel, Duration::from_millis(150), libc::SIGINT);
        assert!(
            !wait_for_input(quiet.as_raw_fd(), &cancel),
            "interrupted, not ready"
        );
        let waited = started.elapsed();
        setter.join().unwrap();
        assert!(waited >= Duration::from_millis(150), "{waited:?}");
        assert!(
            waited < Duration::from_millis(600),
            "about one slice after the signal: {waited:?}"
        );
    }

    #[test]
    fn a_token_already_set_never_waits() {
        let (quiet, _writer) = UnixStream::pair().unwrap();
        let cancel = Cancel::new();
        cancel.request(libc::SIGTERM);
        let started = Instant::now();
        assert!(!wait_for_input(quiet.as_raw_fd(), &cancel));
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    /// A descriptor number that is not open, for a poll that answers `POLLNVAL` at once, as
    /// macOS's does on `/dev/tty`. A duplicate of a descriptor this test owns, taken well above
    /// any descriptor another test opens and closed right away, so nothing else holds the
    /// number meanwhile. It stays a bare number: no `BorrowedFd` or `OwnedFd` ever names it.
    fn closed_fd() -> libc::c_int {
        let (own, _peer) = UnixStream::pair().unwrap();
        // SAFETY: F_DUPFD only allocates a new descriptor, which is closed right after.
        let fd = unsafe { libc::fcntl(own.as_raw_fd(), libc::F_DUPFD, 500) };
        assert!(fd >= 500, "{}", io::Error::last_os_error());
        // SAFETY: `fd` was just opened here and nothing else owns it.
        unsafe { libc::close(fd) };
        fd
    }

    #[test]
    fn a_poll_that_returns_at_once_without_input_never_spins_and_then_lets_the_read_decide() {
        // macOS's poll(2) answers POLLNVAL at once on /dev/tty. Taking that for readiness made
        // every prompt a busy loop: the read said WouldBlock, and the loop came straight back.
        // Now the wait sleeps out its slice and reports ready, so the non-blocking read decides
        // (WouldBlock goes round again, one read per slice; end of input and errors decline).
        let fd = closed_fd();
        let cancel = Cancel::new();
        let started = Instant::now();
        assert!(wait_for_input(fd, &cancel), "ready, for the read to decide");
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(SLICE_MS as u64 - 5)
                && waited < Duration::from_millis(400),
            "no sooner than about one slice: {waited:?}"
        );
        // A signal during that sleep is still seen at once: interrupted, not ready.
        let setter = set_after(&cancel, Duration::from_millis(30), libc::SIGINT);
        assert!(!wait_for_input(fd, &cancel), "the token is looked at again");
        setter.join().unwrap();
    }

    #[test]
    fn a_terminal_that_cannot_be_opened_by_name_falls_back_to_the_generic_one() {
        // `su` in the same pty leaves the device `crw--w----`, so opening it by name is EACCES.
        let pty = pty();
        let gone = Path::new("/nonexistent/ttys999");
        assert!(open_terminal(gone).is_err());
        assert!(open_terminal_or(gone, &pty.path).is_ok());
        assert!(
            open_terminal_or(gone, gone).is_err(),
            "both failing is an error"
        );
    }

    #[test]
    fn the_terminal_a_prompt_opens_is_the_device_its_stream_is() {
        let pty = pty();
        // The slave, as a stream a person types at: dup'ed, as stdin or stderr would be.
        let stream = pty.slave.try_clone().unwrap();
        assert_eq!(device_of(stream.as_fd()), Some(pty.path.clone()));
        let (quiet, _writer) = UnixStream::pair().unwrap();
        assert_eq!(device_of(quiet.as_fd()), None, "a socket is no terminal");
        // The first stream that is a terminal wins; none at all falls back to /dev/tty.
        assert_eq!(terminal_among(&[quiet.as_fd(), stream.as_fd()]), pty.path);
        assert_eq!(terminal_among(&[quiet.as_fd()]), PathBuf::from(TERMINAL));
        // And it opens, as a prompt opens it, and reads what is typed.
        let tty = open_terminal(&terminal_among(&[stream.as_fd()])).unwrap();
        (&pty.master).write_all(b"y\n").unwrap();
        assert_eq!(
            read_line(&tty, &Cancel::new()).unwrap(),
            LineRead::Line("y".into())
        );
    }

    #[test]
    fn input_ready_or_closed_ends_the_wait_and_the_read_decides() {
        let (ready, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"y\n").unwrap();
        assert!(wait_for_input(ready.as_raw_fd(), &Cancel::new()));
        let (closed, writer) = UnixStream::pair().unwrap();
        drop(writer);
        assert!(
            wait_for_input(closed.as_raw_fd(), &Cancel::new()),
            "end of input: the read sees it and declines"
        );
    }

    #[test]
    fn every_prompt_declines_without_asking_once_the_token_is_set() {
        let cancel = Cancel::new();
        cancel.request(libc::SIGHUP);
        let mut p = TtyPrompter::new(cancel);
        assert!(
            !p.confirm("Replace it?", true),
            "a decline, never the default yes"
        );
        assert_eq!(p.choose("Which account?", &["a".into(), "b".into()]), None);
        assert_eq!(
            p.secret("Token: "),
            None,
            "the terminal is never even opened"
        );
    }

    /// A pseudo-terminal: `master` is the person's keyboard and screen, `slave` the terminal
    /// itself, and `path` its name, which the readers open afresh as a prompt opens
    /// `/dev/tty`. These tests need a real pty, and fail, never skip, where none can be opened:
    /// some sandboxes block `/dev/ptmx`, and CI runners provide one.
    struct Pty {
        master: File,
        slave: File,
        path: PathBuf,
    }

    impl Pty {
        /// The terminal, opened as `TtyPrompter` opens it: a description of its own,
        /// non-blocking.
        fn open(&self) -> File {
            open_terminal(&self.path).unwrap()
        }
    }

    fn pty() -> Pty {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: both out-pointers are valid for one write each; a null name, settings and
        // window size ask openpty for its defaults.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(
            rc,
            0,
            "these tests need a pseudo-terminal, and openpty failed: {}",
            io::Error::last_os_error()
        );
        // SAFETY: openpty returned 0, so both are open descriptors that nothing else owns.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        let mut name = [0 as libc::c_char; 256];
        // SAFETY: `name` is writable for the length passed, and `slave` is open for the call.
        let rc = unsafe { libc::ttyname_r(slave.as_raw_fd(), name.as_mut_ptr(), name.len()) };
        assert_eq!(rc, 0, "ttyname_r: {}", io::Error::from_raw_os_error(rc));
        // SAFETY: ttyname_r returned 0, so `name` holds a NUL-terminated path.
        let name = unsafe { CStr::from_ptr(name.as_ptr()) };
        Pty {
            master: File::from(master),
            slave: File::from(slave),
            path: PathBuf::from(OsStr::from_bytes(name.to_bytes())),
        }
    }

    /// The terminal settings these tests compare: the flags and the control characters.
    type Flags = (
        libc::tcflag_t,
        libc::tcflag_t,
        libc::tcflag_t,
        libc::tcflag_t,
        [libc::cc_t; libc::NCCS],
    );

    fn flags(tty: &File) -> Flags {
        let t = termios(tty.as_fd()).unwrap();
        (t.c_iflag, t.c_oflag, t.c_cflag, t.c_lflag, t.c_cc)
    }

    /// Whether `f` has something to read within `ms`.
    fn readable(f: &File, ms: libc::c_int) -> bool {
        let mut fds = libc::pollfd {
            fd: f.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid, writable `pollfd`, and `f` keeps its descriptor open for the call.
        unsafe { libc::poll(&mut fds, 1, ms) > 0 }
    }

    /// What the terminal has shown on its screen since the last look, waiting at most 200 ms for
    /// more each time.
    fn shown(master: &File) -> Vec<u8> {
        let (mut out, mut chunk, mut screen) = (Vec::new(), [0u8; 256], master);
        while readable(master, 200) {
            match screen.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => out.extend_from_slice(&chunk[..n]),
            }
        }
        out
    }

    /// Types `keys` at the terminal, and waits until it has taken them in: its echo shows it.
    fn typed(master: &File, keys: &[u8]) {
        let mut keyboard = master;
        keyboard.write_all(keys).unwrap();
        let echo = String::from_utf8_lossy(keys).trim_end().to_owned();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut screen = Vec::new();
        while !String::from_utf8_lossy(&screen).contains(&echo) {
            assert!(
                Instant::now() < deadline,
                "the terminal never took the input"
            );
            screen.extend(shown(master));
        }
    }

    type Reader = fn(&File, &Cancel) -> io::Result<LineRead>;

    /// A read with its window between readiness and the read (`read_line_between`).
    type WindowReader = fn(&File, &Cancel, &mut dyn FnMut()) -> io::Result<LineRead>;

    /// The line prompts' read and the secret prompt's.
    fn readers() -> [(&'static str, Reader); 2] {
        [("line", read_line), ("secret", read_secret)]
    }

    /// Runs `read` on its own thread and returns what it read. Fails the test, never hangs it,
    /// when the read has not returned within 3 s: one that blocks never does.
    fn within_3s(read: impl FnOnce() -> io::Result<LineRead> + Send + 'static) -> LineRead {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(read());
        });
        rx.recv_timeout(Duration::from_secs(3))
            .expect("the read never returned: it blocked")
            .unwrap()
    }

    #[test]
    fn a_signal_ends_either_read_and_leaves_the_terminal_as_it_was() {
        // §14.1: every prompt, the no-echo one included, is a cancellation point. A Ctrl-C
        // reaches the handler as SIGINT, and a SIGTERM or SIGHUP from elsewhere arrives the
        // same way: each only sets the token. Half an answer is typed, and no newline ever is.
        for (name, read) in readers() {
            for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
                let pty = pty();
                let tty = pty.open();
                let before = flags(&pty.slave);
                assert_ne!(before.3 & libc::ECHO, 0, "a fresh pty echoes");
                (&pty.master).write_all(b"sk-ant-half").unwrap();
                let cancel = Cancel::new();
                let started = Instant::now();
                let setter = set_after(&cancel, Duration::from_millis(150), signal);
                let got = read(&tty, &cancel).unwrap();
                let waited = started.elapsed();
                setter.join().unwrap();
                assert_eq!(got, LineRead::Interrupted, "{name}, signal {signal}");
                assert!(
                    waited >= Duration::from_millis(150) && waited < Duration::from_millis(600),
                    "{name}, signal {signal}: about one slice after it: {waited:?}"
                );
                assert_eq!(
                    flags(&pty.slave),
                    before,
                    "{name}, signal {signal}: the terminal is as it was"
                );
            }
        }
    }

    #[test]
    fn input_flushed_between_readiness_and_the_read_never_blocks_either_read() {
        // The window between `poll` and `read`: a whole line makes the terminal ready, then a
        // Ctrl-C (ISIG) empties its input before the read, and the handler sets the token. A
        // blocking read would wait for input that is gone; this one reports `WouldBlock` and
        // goes back to the token.
        let readers: [(&str, WindowReader); 2] = [
            ("line", |tty, cancel, between| {
                read_line_between(tty, cancel, between)
            }),
            ("secret", |tty, cancel, between| {
                read_secret_between(tty, cancel, between)
            }),
        ];
        for (name, read) in readers {
            let pty = pty();
            let tty = pty.open();
            typed(&pty.master, b"yes\n");
            let cancel = Cancel::new();
            let windows = Arc::new(AtomicUsize::new(0));
            let (handler, seen) = (cancel.clone(), windows.clone());
            let started = Instant::now();
            let got = within_3s(move || {
                let mut between = || {
                    // What the terminal does on a Ctrl-C: it discards its input queue.
                    // SAFETY: tcflush(3) only acts on the terminal behind `tty`, open here.
                    let _ = unsafe { libc::tcflush(tty.as_raw_fd(), libc::TCIFLUSH) };
                    handler.request(libc::SIGINT);
                    seen.fetch_add(1, Ordering::SeqCst);
                };
                read(&tty, &cancel, &mut between)
            });
            assert_eq!(got, LineRead::Interrupted, "{name}");
            assert_eq!(
                windows.load(Ordering::SeqCst),
                1,
                "{name}: the read went through the window once"
            );
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "{name}: {:?}",
                started.elapsed()
            );
        }
    }

    #[test]
    fn an_interrupted_read_discards_what_was_typed() {
        // Half an answer left in the terminal's queue would reach the shell's prompt, and its
        // screen, once tagteam exits.
        for (name, read) in readers() {
            let pty = pty();
            let tty = pty.open();
            typed(&pty.master, b"sk-ant-half");
            let cancel = Cancel::new();
            cancel.request(libc::SIGTERM);
            assert_eq!(
                read(&tty, &cancel).unwrap(),
                LineRead::Interrupted,
                "{name}"
            );
            (&pty.master).write_all(b"\n").unwrap();
            assert_eq!(
                read(&tty, &Cancel::new()).unwrap(),
                LineRead::Line(String::new()),
                "{name}: only the newline typed afterwards is left"
            );
        }
    }

    #[test]
    fn a_line_longer_than_one_read_is_read_whole() {
        let pty = pty();
        let tty = pty.open();
        let long = "a".repeat(300);
        (&pty.master)
            .write_all(format!("{long}\n").as_bytes())
            .unwrap();
        assert_eq!(
            read_line(&tty, &Cancel::new()).unwrap(),
            LineRead::Line(long)
        );
    }

    #[test]
    fn a_secret_is_read_with_echo_off_and_the_terminal_s_signals_on() {
        let pty = pty();
        let tty = pty.open();
        let before = flags(&pty.slave);
        let cancel = Cancel::new();
        let reader = thread::spawn(move || read_secret(&tty, &cancel).unwrap());
        // The barrier: echo going off says the read is under way, before anything is typed.
        let deadline = Instant::now() + Duration::from_secs(5);
        let during = loop {
            let now = flags(&pty.slave);
            if now.3 & libc::ECHO == 0 {
                break now;
            }
            assert!(Instant::now() < deadline, "echo never went off");
            thread::sleep(Duration::from_millis(5));
        };
        assert_ne!(
            during.3 & libc::ISIG,
            0,
            "Ctrl-C still raises SIGINT, so it is never a byte of the secret"
        );
        assert_ne!(
            during.3 & libc::ICANON,
            0,
            "the terminal still edits the line"
        );
        (&pty.master).write_all(b"sk-ant-secret\n").unwrap();
        assert_eq!(
            reader.join().unwrap(),
            LineRead::Line("sk-ant-secret".into())
        );
        assert_eq!(flags(&pty.slave), before, "echo is back on");
        assert!(
            !String::from_utf8_lossy(&shown(&pty.master)).contains("secret"),
            "nothing typed was shown"
        );
    }

    #[test]
    fn a_ctrl_c_typed_at_the_secret_prompt_is_never_part_of_the_secret() {
        // `ISIG` stays on, so the terminal takes ^C itself: it discards the line typed so far
        // and signals its foreground group (nobody here: the pty is no one's controlling
        // terminal; in tagteam, the handler sets the token).
        let pty = pty();
        let tty = pty.open();
        let cancel = Cancel::new();
        let reader = thread::spawn(move || read_secret(&tty, &cancel).unwrap());
        (&pty.master).write_all(b"wrong\x03").unwrap();
        (&pty.master).write_all(b"right\n").unwrap();
        assert_eq!(reader.join().unwrap(), LineRead::Line("right".into()));
    }

    #[test]
    fn end_of_input_before_a_line_reads_as_the_end() {
        for (name, read) in readers() {
            let pty = pty();
            let tty = pty.open();
            (&pty.master).write_all(&[4]).unwrap(); // Ctrl-D on an empty line
            assert_eq!(read(&tty, &Cancel::new()).unwrap(), LineRead::End, "{name}");
        }
    }
}
