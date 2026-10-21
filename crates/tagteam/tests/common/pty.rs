//! A program on a pseudo-terminal, and a small terminal emulator to read back what it shows
//! (TUI spec §11 "End to end"). `tui_pty.rs` and `perf.rs` run the TUI through it. `openpty`
//! fails with EPERM inside a sandbox that denies it, so the tests that use it run outside one.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthChar;

/// The terminal every test opens: 120 columns by 40 rows (TUI spec §11).
pub const COLS: u16 = 120;
pub const ROWS: u16 = 40;

/// The cell a wide character's right half takes.
const WIDE: char = '\0';

/// A program on a pseudo-terminal of `COLS`×`ROWS`, started as a terminal emulator starts a
/// shell: in a session of its own whose controlling terminal the pty is, so the terminal's
/// Ctrl-C and Ctrl-\ reach its foreground process group. A thread reads everything it writes.
pub struct Pty {
    pub child: Child,
    /// When it was spawned.
    pub started: Instant,
    master: File,
    slave: OwnedFd,
    written: Arc<Mutex<Written>>,
    /// Its size now, in columns and rows: `COLS`×`ROWS` until `resize`.
    size: Mutex<(u16, u16)>,
}

/// What the program wrote, and when each chunk of it was read.
#[derive(Default)]
struct Written {
    bytes: Vec<u8>,
    /// When each chunk arrived, and the length of `bytes` with it.
    chunks: Vec<(Instant, usize)>,
}

/// How a program on a pty ended.
pub struct Finished {
    pub status: ExitStatus,
    /// What the terminal shows once it has exited.
    pub screen: Screen,
    /// Everything it wrote.
    pub output: Vec<u8>,
    /// The terminal's local modes as it left them (`c_lflag`).
    pub lflag: libc::tcflag_t,
}

impl Pty {
    /// Starts `cmd` with the pty as its stdout and stderr, and as its stdin when
    /// `stdin_is_terminal` (else stdin is null).
    pub fn spawn(mut cmd: Command, stdin_is_terminal: bool) -> Pty {
        let (master, slave) = open_pty();
        let stdin = if stdin_is_terminal {
            Stdio::from(slave.try_clone().unwrap())
        } else {
            Stdio::null()
        };
        cmd.env("TERM", "xterm-256color")
            .stdin(stdin)
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()));
        // SAFETY: the closure runs in the child between fork and exec, and calls only signal(2),
        // sigemptyset(3), sigprocmask(2), setsid(2) and ioctl(2), which are async-signal-safe.
        // Its stdout is the pty's slave by then.
        unsafe {
            cmd.pre_exec(|| {
                // As a terminal emulator starts a shell: default dispositions and an empty
                // mask, whatever this process inherited. A SIGINT or SIGQUIT ignored on entry
                // (a backgrounded `cargo test`, nohup) would pass down to the TUI, `run` and
                // `claude`, and production keeps an inherited SIG_IGN.
                for signal in [libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGHUP] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                let mut none: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut none);
                libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
                if libc::setsid() < 0 || libc::ioctl(1, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let started = Instant::now();
        let child = cmd.spawn().unwrap();
        let master = File::from(master);
        let written = Arc::new(Mutex::new(Written::default()));
        let (mut from, into) = (master.try_clone().unwrap(), Arc::clone(&written));
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match from.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let arrived = Instant::now();
                        let mut w = into.lock().unwrap();
                        w.bytes.extend_from_slice(&buf[..n]);
                        let end = w.bytes.len();
                        w.chunks.push((arrived, end));
                    }
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    // EIO once every slave descriptor is closed (Linux).
                    Err(_) => break,
                }
            }
        });
        Pty {
            child,
            started,
            master,
            slave,
            written,
            size: Mutex::new((COLS, ROWS)),
        }
    }

    /// Types `keys` at the terminal.
    pub fn type_keys(&self, keys: &[u8]) {
        (&self.master).write_all(keys).unwrap();
    }

    /// Resizes the terminal to `cols`×`rows`, as a terminal emulator does when its window
    /// changes size: the kernel sends SIGWINCH to the foreground process group. `screen` then
    /// emulates at the new size.
    pub fn resize(&self, cols: u16, rows: u16) {
        set_size(&self.master, cols, rows);
        *self.size.lock().unwrap() = (cols, rows);
    }

    /// Sends `signal` to the program alone, as `kill` does.
    pub fn signal(&self, signal: i32) {
        // SAFETY: kill(2) reads no memory of ours. The child is not reaped yet, so its pid
        // names it and no other process.
        let rc = unsafe { libc::kill(self.child.id() as libc::pid_t, signal) };
        assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());
    }

    /// What the terminal shows now.
    pub fn screen(&self) -> Screen {
        let (cols, rows) = *self.size.lock().unwrap();
        Screen::of(&self.written.lock().unwrap().bytes, cols, rows)
    }

    /// Everything the program has written so far.
    pub fn output(&self) -> Vec<u8> {
        self.written.lock().unwrap().bytes.clone()
    }

    /// Polls every 10 ms until `ready` holds of what the terminal shows, and returns that.
    /// Fails the test if the program exits first, or if `within` passes.
    pub fn wait_for(
        &mut self,
        within: Duration,
        what: &str,
        ready: impl Fn(&Screen) -> bool,
    ) -> Screen {
        let deadline = Instant::now() + within;
        loop {
            let screen = self.screen();
            if ready(&screen) {
                return screen;
            }
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("it exited ({status}) before {what}:\n{}", screen.text());
            }
            assert!(
                Instant::now() < deadline,
                "it never got to {what}:\n{}",
                screen.text()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Polls every 10 ms until `ready` holds. Fails the test if the program exits first, or
    /// if `within` passes.
    pub fn wait_until(&mut self, within: Duration, what: &str, ready: impl Fn() -> bool) {
        let deadline = Instant::now() + within;
        while !ready() {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!(
                    "it exited ({status}) before {what}:\n{}",
                    self.screen().text()
                );
            }
            assert!(
                Instant::now() < deadline,
                "it never got to {what}:\n{}",
                self.screen().text()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Polls every 10 ms until the program has written `needle`. Fails the test if the program
    /// exits first, or if `within` passes.
    pub fn wait_for_output(&mut self, within: Duration, what: &str, needle: &[u8]) {
        let deadline = Instant::now() + within;
        while !self.output().windows(needle.len()).any(|w| w == needle) {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!(
                    "it exited ({status}) before {what}:\n{}",
                    self.screen().text()
                );
            }
            assert!(
                Instant::now() < deadline,
                "it never got to {what}:\n{}",
                self.screen().text()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// When what `ready` holds of was first on the terminal: the arrival of the chunk that put
    /// it there. Each chunk is emulated at the size `screen` uses.
    pub fn first_shown(&self, ready: impl Fn(&Screen) -> bool) -> Option<Instant> {
        let (cols, rows) = *self.size.lock().unwrap();
        let written = self.written.lock().unwrap();
        written
            .chunks
            .iter()
            .find(|(_, end)| ready(&Screen::of(&written.bytes[..*end], cols, rows)))
            .map(|(at, _)| *at)
    }

    /// The terminal's local modes (`c_lflag`) now. Once the program, the session's leader, has
    /// exited, macOS revokes the slave descriptor this harness holds, and its master reads the
    /// modes the two ends share; Linux keeps the slave usable, and its master has modes of its
    /// own. So the slave is asked first, then the master.
    fn local_modes(&self) -> libc::tcflag_t {
        for fd in [self.slave.as_raw_fd(), self.master.as_raw_fd()] {
            // SAFETY: an all-zero termios is a valid value for tcgetattr(3) to overwrite.
            let mut modes: libc::termios = unsafe { std::mem::zeroed() };
            // SAFETY: `fd` is open, and `modes` is one writable termios.
            if unsafe { libc::tcgetattr(fd, &mut modes) } == 0 {
                return modes.c_lflag;
            }
        }
        panic!("tcgetattr: {}", std::io::Error::last_os_error())
    }

    /// How many bytes the master holds that the reading thread has not taken yet.
    fn unread(&self) -> libc::c_int {
        let mut n: libc::c_int = 0;
        // SAFETY: FIONREAD writes one int through the pointer, which is valid for the call.
        let rc = unsafe { libc::ioctl(self.master.as_raw_fd(), libc::FIONREAD as _, &mut n) };
        if rc == 0 { n } else { 0 }
    }

    /// Waits for the program to exit, then for the last of its output, and says how it ended.
    /// Fails the test if it still runs after `within`.
    pub fn finish(mut self, within: Duration) -> Finished {
        let deadline = Instant::now() + within;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "it was still running {within:?} later:\n{}",
                self.screen().text()
            );
            thread::sleep(Duration::from_millis(10));
        };
        // Its last bytes may still be in the pty: read until the master has nothing left and
        // the count has held for three polls, within 2 s.
        let ceiling = Instant::now() + Duration::from_secs(2);
        let (mut seen, mut steady) = (usize::MAX, 0);
        while steady < 3 && Instant::now() < ceiling {
            thread::sleep(Duration::from_millis(20));
            let now = self.written.lock().unwrap().bytes.len();
            steady = if now == seen && self.unread() == 0 {
                steady + 1
            } else {
                0
            };
            seen = now;
        }
        Finished {
            status,
            screen: self.screen(),
            output: self.output(),
            lflag: self.local_modes(),
        }
    }
}

impl Drop for Pty {
    /// A test that failed part way leaves no program running.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// A pseudo-terminal pair of `COLS`×`ROWS`, neither end inherited by children.
fn open_pty() -> (OwnedFd, OwnedFd) {
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty(3) writes two descriptors into the integers passed; the name, termios
    // and winsize arguments may be null.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: openpty succeeded, so both are open descriptors that nothing else owns.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    for fd in [&master, &slave] {
        // SAFETY: fcntl(2) with F_SETFD takes an open descriptor and an integer.
        let rc = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        assert_eq!(rc, 0, "fcntl: {}", std::io::Error::last_os_error());
    }
    set_size(&master, COLS, ROWS);
    (master, slave)
}

/// Sets the size of the pty whose master is `master`. Once it has a foreground process group,
/// the kernel sends that group SIGWINCH.
fn set_size(master: &impl AsRawFd, cols: u16, rows: u16) {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `size` is one valid winsize, which ioctl(2) reads for the duration of the call.
    let rc = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) };
    assert_eq!(rc, 0, "TIOCSWINSZ: {}", std::io::Error::last_os_error());
}

/// A terminal's two screens as a small emulator reads what a program wrote: text, cursor moves,
/// clears and the alternate screen. Colours and attributes are ignored, and so is every
/// sequence crossterm and ratatui do not send. Enough to read back what they draw.
#[derive(Clone)]
pub struct Screen {
    cols: usize,
    rows: usize,
    main: Vec<Vec<char>>,
    alt: Vec<Vec<char>>,
    on_alt: bool,
    row: usize,
    col: usize,
    saved: (usize, usize),
}

impl Screen {
    /// The screen after `bytes`, on a terminal of `cols`×`rows`.
    pub fn of(bytes: &[u8], cols: u16, rows: u16) -> Screen {
        let (cols, rows) = (usize::from(cols), usize::from(rows));
        let mut screen = Screen {
            cols,
            rows,
            main: blank(cols, rows),
            alt: blank(cols, rows),
            on_alt: false,
            row: 0,
            col: 0,
            saved: (0, 0),
        };
        screen.feed(&String::from_utf8_lossy(bytes));
        screen
    }

    /// The screen shown now, a line per row, without trailing blanks.
    pub fn text(&self) -> String {
        let grid = if self.on_alt { &self.alt } else { &self.main };
        grid.iter()
            .map(|row| {
                let line: String = row.iter().filter(|&&c| c != WIDE).collect();
                line.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn contains(&self, text: &str) -> bool {
        self.text().contains(text)
    }

    /// Whether the alternate screen is the one shown.
    pub fn on_alt(&self) -> bool {
        self.on_alt
    }

    fn grid(&mut self) -> &mut Vec<Vec<char>> {
        if self.on_alt {
            &mut self.alt
        } else {
            &mut self.main
        }
    }

    fn feed(&mut self, text: &str) {
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => match chars.next() {
                    Some('[') => {
                        let mut params = String::new();
                        for c in chars.by_ref() {
                            if ('\x40'..='\x7e').contains(&c) {
                                self.csi(&params, c);
                                break;
                            }
                            params.push(c);
                        }
                    }
                    // An operating system command (`ESC ] … BEL` or `… ESC \`), such as a
                    // colour query: none of it is text.
                    Some(']') => {
                        while let Some(c) = chars.next() {
                            if c == '\x07' {
                                break;
                            }
                            if c == '\x1b' {
                                chars.next();
                                break;
                            }
                        }
                    }
                    Some('7') => self.saved = (self.row, self.col),
                    Some('8') => (self.row, self.col) = self.saved,
                    // A character set (`ESC ( B`): its one argument goes too.
                    Some('(' | ')') => {
                        chars.next();
                    }
                    _ => {}
                },
                '\r' => self.col = 0,
                '\n' => self.line_feed(),
                '\x08' => self.col = self.col.saturating_sub(1),
                c if c.is_control() => {}
                c => self.put(c),
            }
        }
    }

    fn csi(&mut self, params: &str, last: char) {
        let private = params.starts_with('?');
        let numbers: Vec<usize> = params
            .trim_start_matches('?')
            .split(';')
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        // Parameter `i`, or `default` for one that is missing or zero.
        let at = |i: usize, default: usize| {
            numbers
                .get(i)
                .copied()
                .filter(|&n| n > 0)
                .unwrap_or(default)
        };
        let mode = numbers.first().copied().unwrap_or(0);
        let (last_row, last_col) = (self.rows - 1, self.cols - 1);
        match (private, last) {
            (true, 'h') if numbers.contains(&1049) => {
                if !self.on_alt {
                    self.saved = (self.row, self.col);
                    self.alt = blank(self.cols, self.rows);
                    self.on_alt = true;
                }
            }
            (true, 'l') if numbers.contains(&1049) => {
                if self.on_alt {
                    self.on_alt = false;
                    (self.row, self.col) = self.saved;
                }
            }
            (false, 'H' | 'f') => {
                self.row = (at(0, 1) - 1).min(last_row);
                self.col = (at(1, 1) - 1).min(last_col);
            }
            (false, 'A') => self.row = self.row.saturating_sub(at(0, 1)),
            (false, 'B') => self.row = (self.row + at(0, 1)).min(last_row),
            (false, 'C') => self.col = (self.col + at(0, 1)).min(last_col),
            (false, 'D') => self.col = self.col.saturating_sub(at(0, 1)),
            (false, 'G') => self.col = (at(0, 1) - 1).min(last_col),
            (false, 'd') => self.row = (at(0, 1) - 1).min(last_row),
            (false, 'J') => {
                let cursor = (self.row, self.col);
                for (r, line) in self.grid().iter_mut().enumerate() {
                    for (c, cell) in line.iter_mut().enumerate() {
                        let erased = match mode {
                            0 => (r, c) >= cursor,
                            1 => (r, c) <= cursor,
                            _ => true,
                        };
                        if erased {
                            *cell = ' ';
                        }
                    }
                }
            }
            (false, 'K') => {
                let (row, col) = (self.row, self.col);
                for (c, cell) in self.grid()[row].iter_mut().enumerate() {
                    let erased = match mode {
                        0 => c >= col,
                        1 => c <= col,
                        _ => true,
                    };
                    if erased {
                        *cell = ' ';
                    }
                }
            }
            // Colours, attributes, the cursor's visibility and the rest change no text.
            _ => {}
        }
    }

    fn put(&mut self, c: char) {
        let width = c.width().unwrap_or(0);
        if width == 0 {
            return;
        }
        // A character past the last column wraps, as a terminal's autowrap does.
        if self.col + width > self.cols {
            self.col = 0;
            self.line_feed();
        }
        let (row, col) = (self.row, self.col);
        let line = &mut self.grid()[row];
        line[col] = c;
        if width == 2 {
            line[col + 1] = WIDE;
        }
        self.col += width;
    }

    fn line_feed(&mut self) {
        if self.row + 1 < self.rows {
            self.row += 1;
            return;
        }
        let cols = self.cols;
        let grid = self.grid();
        grid.remove(0);
        grid.push(vec![' '; cols]);
    }
}

fn blank(cols: usize, rows: usize) -> Vec<Vec<char>> {
    vec![vec![' '; cols]; rows]
}
