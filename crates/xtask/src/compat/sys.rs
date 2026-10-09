//! The processes compat starts. Every `claude` and `tagteam` gets an environment built from
//! nothing and checked by the guard before it starts (B.70), runs in a process group of its own
//! so that a timeout ends all of it, and has a deadline. No process group outlives its handle:
//! when a `Running` or `Pty` goes, its child is reaped and its group probed until empty, ended
//! if needed (an early `?`, a panic, a descendant left running), and `quiesce` proves every
//! group empty before teardown touches a credential. SIGINT, SIGTERM and SIGHUP only set the
//! cancel token (`catch_signals`); every wait here is a cancellation point, so a signal unwinds
//! the run through that same cleanup.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Read as _, Write as _};
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGTSTP};
use tagteam_provider::{Cancel, term};

use super::guard::Roots;
use super::report::Redactor;

/// The harness itself failed: a check that meets one proves nothing (exit 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessError(pub String);

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<io::Error> for HarnessError {
    fn from(e: io::Error) -> Self {
        HarnessError(e.to_string())
    }
}

impl From<super::guard::Refusal> for HarnessError {
    fn from(r: super::guard::Refusal) -> Self {
        HarnessError(format!("refused (B.70): {r}"))
    }
}

pub fn harness(message: impl Into<String>) -> HarnessError {
    HarnessError(message.into())
}

/// `s` quoted for `/bin/sh`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The first executable `name` on `path`.
pub fn which_in(name: &str, path: &OsStr) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// A terminal transcript as text: escape sequences and control characters other than newline
/// and tab removed, carriage returns read as line ends.
pub fn strip_ansi(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI: parameters, then one final byte in @..~.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC: up to BEL or ESC \.
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    out.push('\n');
                }
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// The last `n` characters of `s`.
pub fn tail(s: &str, n: usize) -> String {
    let count = s.chars().count();
    s.chars().skip(count.saturating_sub(n)).collect()
}

/// Sends `signal` (`INT`, `TERM`, `HUP`, `KILL`) to `pid`, or to its process group when
/// `group`. Through `/bin/kill`, since `xtask` forbids `unsafe` (its few libc calls are
/// `tagteam_provider::term`'s).
pub fn signal(pid: u32, signal: &str, group: bool) -> bool {
    let target = if group {
        format!("-{pid}")
    } else {
        pid.to_string()
    };
    Command::new("/bin/kill")
        .args([&format!("-{signal}"), "--", &target])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The process's cancel token, the workspace's own (§14.1): `catch_signals` has SIGINT, SIGTERM
/// and SIGHUP store their number in it, and every wait point reads it.
static CANCEL: LazyLock<Cancel> = LazyLock::new(Cancel::new);

/// The signals that end a run, through its cleanup (`catch_signals`).
pub const CAUGHT: [i32; 3] = [SIGINT, SIGTERM, SIGHUP];

pub fn cancel() -> &'static Cancel {
    &CANCEL
}

/// Has each of `signals` store its number in `token` (signal-hook's flag, as `tagteam`'s own
/// handlers do), so a signal no longer ends the harness where it lands: the run stops at its
/// next wait and unwinds. `run` and `compat login` call it with `cancel()` and `CAUGHT`.
pub fn catch_signals(token: &Cancel, signals: &[i32]) -> io::Result<()> {
    for &signal in signals {
        signal_hook::flag::register_usize(signal, token.cell(), signal as usize)?;
    }
    Ok(())
}

pub fn signal_name(n: i32) -> String {
    match n {
        SIGHUP => "SIGHUP".to_owned(),
        SIGINT => "SIGINT".to_owned(),
        SIGTERM => "SIGTERM".to_owned(),
        n => format!("signal {n}"),
    }
}

/// What a cancellation point that found signal `n` returns.
pub fn interrupted(n: i32) -> HarnessError {
    harness(format!("interrupted by {}", signal_name(n)))
}

/// Polls `done` every 20 ms until it holds or `timeout` passes, or, with `token`, until a
/// signal is recorded in it.
fn poll(timeout: Duration, token: Option<&Cancel>, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline || token.is_some_and(|t| t.requested().is_some()) {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Polls `done` every 20 ms until it holds or `timeout` passes. A cancellation point: once a
/// signal is recorded (`cancel()`), it gives up at once.
pub fn wait_until(timeout: Duration, done: impl FnMut() -> bool) -> bool {
    poll(timeout, Some(cancel()), done)
}

/// A pause a check makes, cut short by a signal as `wait_until` is.
pub fn pause(d: Duration) {
    wait_until(d, || false);
}

/// The cleanup's own waits, which a signal must not cut short: ending and reaping the groups,
/// and seeing the daemons stop.
pub fn settle(timeout: Duration, done: impl FnMut() -> bool) -> bool {
    poll(timeout, None, done)
}

/// A child's standard output as it printed it. It is for control flow only (`Ran::json`,
/// `Ran::parse`), so
/// it has no `Debug`, `Display` or `Serialize`: nothing can format it, and nothing writes it.
#[derive(Clone)]
struct Raw(Vec<u8>);

/// One finished process. `stdout` and `stderr` are the redacted view of its output, the one
/// every string, report and error is made from; its raw standard output stays inside, for
/// `json` alone.
#[derive(Clone)]
pub struct Ran {
    pub what: String,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub seconds: f64,
    pub timed_out: bool,
    raw: Raw,
}

impl fmt::Debug for Ran {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ran")
            .field("what", &self.what)
            .field("code", &self.code)
            .field("stdout", &String::from_utf8_lossy(&self.stdout))
            .field("stderr", &String::from_utf8_lossy(&self.stderr))
            .field("seconds", &self.seconds)
            .field("timed_out", &self.timed_out)
            .finish_non_exhaustive()
    }
}

impl Ran {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// Standard output as it may be shown: redacted.
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Standard output as one JSON value, if it is one, parsed from the raw view: for control
    /// flow, deciding and comparing, never for evidence or an error, which take `stdout`.
    pub fn json(&self) -> Option<Value> {
        serde_json::from_slice(self.raw.0.trim_ascii()).ok()
    }

    /// Standard output's raw text, lent to `parse` for control flow (`claude --version`'s
    /// version, say). What `parse` returns must not carry the text on: a diagnostic takes
    /// `stdout_text`.
    pub fn parse<T>(&self, parse: impl FnOnce(&str) -> T) -> T {
        parse(&String::from_utf8_lossy(&self.raw.0))
    }

    /// For a report: the command, its exit, its time and the end of its standard error, which
    /// `Running::wait` redacted as it captured it, so any string this is formatted into carries
    /// placeholders only.
    pub fn summary(&self) -> Value {
        json!({
            "command": self.what,
            "exit": self.code,
            "timedOut": self.timed_out,
            "seconds": (self.seconds * 10.0).round() / 10.0,
            "stderr": tail(&String::from_utf8_lossy(&self.stderr), 400),
        })
    }
}

pub fn drain(pipe: Option<impl io::Read + Send + 'static>) -> Receiver<Vec<u8>> {
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

fn reaped(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(Some(_)))
}

/// Whether any process is left in the process group `pgid`: `kill(-pgid, 0)`, through
/// `/bin/kill`, is empty only on ESRCH ("No such process"). Any other answer counts as a
/// process left.
fn group_alive(pgid: u32) -> bool {
    alive(&format!("-{pgid}"))
}

/// Whether the process `pid` runs, by the same probe: only ESRCH says it does not.
pub fn process_alive(pid: u32) -> bool {
    alive(&pid.to_string())
}

fn alive(target: &str) -> bool {
    let probe = Command::new("/bin/kill")
        .args(["-0", "--", target])
        .env_clear()
        .env("LC_ALL", "C")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match probe {
        Ok(out) if out.status.success() => true,
        Ok(out) => !String::from_utf8_lossy(&out.stderr).contains("No such process"),
        Err(_) => true,
    }
}

/// A child is done when it is reaped and its process group, whose id is its pid, is empty.
fn done(child: &mut Child) -> bool {
    reaped(child) && !group_alive(child.id())
}

const GRACE: Duration = Duration::from_secs(2);

/// Ends `child`'s process group: SIGTERM while the child itself runs, then, after the grace
/// period, SIGKILL to whatever of the group is left, a descendant that outlived the child
/// included, then up to the grace period again. Whether the child was reaped and its group is
/// empty. The group is signalled only right after its leader was found unreaped or a probe
/// found it holding a process, so its id is still its own.
fn end_group(child: &mut Child) -> bool {
    if done(child) {
        return true;
    }
    let pgid = child.id();
    if !reaped(child) {
        signal(pgid, "TERM", true);
    }
    if settle(GRACE, || done(child)) {
        return true;
    }
    signal(pgid, "KILL", true);
    settle(GRACE, || done(child))
}

/// Starts `cmd` as an interactive child: in a process group of its own (a shell's job control),
/// with the terminal's streams inherited. `std` makes the group before it returns, so the group
/// exists when this does. Wait for it with `wait_interactive`.
pub fn spawn_interactive(cmd: &mut Command) -> io::Result<Child> {
    term::spawn_foreground(cmd)
}

/// The terminal handed to an interactive child's group. Dropping it hands the terminal back to
/// the harness's own group, so no path leaves it with a dead or foreign foreground group. Without
/// a terminal on stdin, or when the harness is not its foreground group, it does nothing.
struct Foreground {
    ours: Option<i32>,
}

impl Foreground {
    fn hand_to(pgid: u32) -> Self {
        let Ok(pgid) = i32::try_from(pgid) else {
            return Self { ours: None };
        };
        let ours = term::own_process_group();
        // The terminal is ours, or the child already took it before its exec
        // (`spawn_foreground`): either way it is handed to the child's group now and back
        // to ours on drop.
        match term::stdin_foreground_group() {
            Some(group) if group == ours || group == pgid => {}
            _ => return Self { ours: None },
        }
        term::set_foreground_group(pgid);
        Self { ours: Some(ours) }
    }
}

impl Drop for Foreground {
    fn drop(&mut self) {
        if let Some(ours) = self.ours.take() {
            term::set_foreground_group(ours);
        }
    }
}

/// Whether `child` leads a process group of its own (`spawn_interactive` made it so). The group
/// is signalled as a whole only then: never the harness's own.
fn leads_its_group(child: &Child) -> bool {
    term::process_group_of(child.id()) == Some(child.id())
}

/// Waits for `child`, started by `spawn_interactive`: a process group of its own, handed the
/// terminal while stdin is one and always handed back, as a cancellation point. A signal
/// recorded in `token` ends it, and so does a leader that ends by SIGINT, SIGTERM or SIGHUP
/// (Ctrl-C reaches its group, not the harness): both return `interrupted`, so the caller
/// unwinds through its cleanup. On every path out, normal exit included, the whole group is
/// sent SIGTERM, then SIGKILL, until `kill(-pgid, 0)` says it is empty, and what left the
/// group (a `setsid`) is swept from the process tree last seen (`end_tree`). A survivor is a
/// harness error, not a silent one.
pub fn wait_interactive(child: &mut Child, token: &Cancel) -> Result<ExitStatus, HarnessError> {
    wait_interactive_as(child, token, false)
}

/// `wait_interactive`; `holds_terminal` says the child's group is to be treated as holding the
/// terminal even when stdin is not one (tests).
fn wait_interactive_as(
    child: &mut Child,
    token: &Cancel,
    holds_terminal: bool,
) -> Result<ExitStatus, HarnessError> {
    let pgid = child.id();
    let own_group = leads_its_group(child);
    let foreground = own_group.then(|| Foreground::hand_to(pgid));
    let holds_terminal = holds_terminal || foreground.as_ref().is_some_and(|f| f.ours.is_some());
    if holds_terminal {
        // Harmless if it runs; a child stopped before the handoff (SIGTTIN) continues.
        signal(pgid, "CONT", true);
    }
    let mut seen = BTreeSet::from([pgid]);
    let mut last_listed = Instant::now();
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(Some(status)),
            Ok(None) => {}
            Err(e) => break Err(harness(format!("could not wait for {pgid}: {e}"))),
        }
        if let Some(n) = token.requested() {
            break Err(interrupted(n));
        }
        if own_group && term::child_is_stopped(pgid) {
            if holds_terminal {
                // Stopped though it holds the terminal (a SIGTTIN lost to the handoff race):
                // resumed, as a shell does for a foreground job.
                signal(pgid, "CONT", true);
            } else {
                // Nobody to resume it for: a stopped command with no terminal never ends.
                token.request(SIGTSTP);
                break Err(interrupted(SIGTSTP));
            }
        }
        // What the tree looks like while the leader lives, for the sweep once it is gone.
        if last_listed.elapsed() >= Duration::from_secs(1) {
            last_listed = Instant::now();
            if let Ok(table) = process_table() {
                seen.extend(tree_of(&table, &seen));
            }
        }
        thread::sleep(Duration::from_millis(20));
    };
    // A leader ended by a terminal Ctrl-C (or SIGTERM, SIGHUP) is the run's cancellation: the
    // token records it, as the harness's own handler would, before any cleanup, so every caller
    // sees `requested()` whatever the cleanup then does.
    if let Ok(Some(status)) = &outcome {
        if let Some(n) = status.signal().filter(|n| CAUGHT.contains(n)) {
            token.request(n);
        }
    }
    // The terminal back first: the sweep below must not run with it held by a dying group.
    drop(foreground);
    let swept = if own_group {
        end_group_of(child, pgid).and_then(|()| end_tree(child, seen))
    } else {
        // Not a group of its own (not started by `spawn_interactive`): the harness's own group
        // is never signalled, only the child and what it started.
        end_tree(child, seen)
    };
    match (outcome, swept) {
        (_, Err(e)) => Err(e),
        (Err(e), Ok(())) => Err(e),
        (Ok(Some(status)), Ok(())) => match status.signal() {
            Some(n) if CAUGHT.contains(&n) => Err(interrupted(n)),
            _ => Ok(status),
        },
        (Ok(None), Ok(())) => Err(harness("the wait ended without a status")),
    }
}

/// Ends the process group `pgid` led by `child`: SIGTERM, up to `GRACE`, SIGKILL, up to `GRACE`
/// again, until `kill(-pgid, 0)` reports it empty, reaping the leader. Nothing is signalled when
/// the group is already empty (the usual end), so a reused id is never hit by a late signal.
fn end_group_of(child: &mut Child, pgid: u32) -> Result<(), HarnessError> {
    let _ = child.try_wait();
    if !group_alive(pgid) {
        return Ok(());
    }
    // A stopped group takes SIGTERM only once continued.
    signal(pgid, "CONT", true);
    signal(pgid, "TERM", true);
    if !settle(GRACE, || {
        let _ = child.try_wait();
        !group_alive(pgid)
    }) {
        signal(pgid, "KILL", true);
        settle(GRACE, || {
            let _ = child.try_wait();
            !group_alive(pgid)
        });
    }
    let _ = child.try_wait();
    if !group_alive(pgid) {
        return Ok(());
    }
    let left = group_members(pgid);
    Err(harness(format!(
        "{} process(es) of the interrupted command's group survived SIGKILL",
        left.max(1)
    )))
}

/// How many processes, zombies not counted, `ps` lists in group `pgid`; 0 if it cannot say.
fn group_members(pgid: u32) -> usize {
    let Ok(out) = Command::new("ps")
        .args(["-A", "-o", "pid=,pgid=,stat="])
        .env_clear()
        .env("PATH", "/bin:/usr/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return 0;
    };
    parse_process_table(&String::from_utf8_lossy(&out.stdout))
        .into_iter()
        .filter(|(_, group, zombie)| *group == pgid && !*zombie)
        .count()
}

/// Passes `end_tree` makes: SIGTERM, then SIGKILL until nothing of the tree is left.
const TREE_PASSES: u32 = 5;

/// `(pid, parent, zombie)` for every process, from `ps -A -o pid=,ppid=,stat=`.
fn process_table() -> io::Result<Vec<(u32, u32, bool)>> {
    let out = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,stat="])
        .env_clear()
        .env("PATH", "/bin:/usr/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other("ps failed"));
    }
    Ok(parse_process_table(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_process_table(out: &str) -> Vec<(u32, u32, bool)> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (pid, ppid) = (f.next()?.parse().ok()?, f.next()?.parse().ok()?);
            Some((pid, ppid, f.next().is_some_and(|s| s.starts_with('Z'))))
        })
        .collect()
}

/// Every process in `table` that descends from one of `roots`, the roots that are in it too.
fn tree_of(table: &[(u32, u32, bool)], roots: &BTreeSet<u32>) -> BTreeSet<u32> {
    let mut tree: BTreeSet<u32> = table
        .iter()
        .filter(|(pid, _, _)| roots.contains(pid))
        .map(|(pid, _, _)| *pid)
        .collect();
    loop {
        let before = tree.len();
        let more: Vec<u32> = table
            .iter()
            .filter(|(_, ppid, _)| tree.contains(ppid))
            .map(|(pid, _, _)| *pid)
            .collect();
        tree.extend(more);
        if tree.len() == before {
            return tree;
        }
    }
}

/// Ends every process in `seen` (the pids of `child` and of what it started, as last listed)
/// and everything that descends from them: what left the group, which killing the group does
/// not reach. The tree is listed again on every pass from what was seen, so a descendant its parent's death re-parented is still found. Each member is
/// signalled alone (never the harness, never a group): SIGTERM, a short grace while the direct
/// child is reaped, then SIGKILL, up to `TREE_PASSES`. A survivor is an error.
fn end_tree(child: &mut Child, mut seen: BTreeSet<u32>) -> Result<(), HarnessError> {
    let me = std::process::id();
    let mut left = 0;
    for pass in 0..TREE_PASSES {
        let Ok(table) = process_table() else {
            // Without a listing nothing can be swept. The group, which needs none, was ended
            // before this.
            return Ok(());
        };
        let _ = child.try_wait();
        let live: Vec<u32> = tree_of(&table, &seen)
            .into_iter()
            .filter(|pid| *pid != me)
            .filter(|pid| {
                // A zombie holds nothing: the direct child is reaped just above, and any other
                // is its dead parent's to reap, or init's.
                !table.iter().any(|(p, _, zombie)| p == pid && *zombie)
            })
            .collect();
        if live.is_empty() {
            return Ok(());
        }
        seen.extend(&live);
        let how = if pass == 0 { "TERM" } else { "KILL" };
        for pid in &live {
            signal(*pid, how, false);
        }
        let grace = if pass == 0 {
            GRACE
        } else {
            Duration::from_millis(500)
        };
        settle(grace, || {
            let _ = child.try_wait();
            process_table().is_ok_and(|t| {
                tree_of(&t, &seen)
                    .iter()
                    .all(|pid| t.iter().any(|(p, _, z)| p == pid && *z))
            })
        });
        left = live.len();
    }
    // The last pass's KILL had its grace: whatever still lists is a survivor.
    let table = process_table().unwrap_or_default();
    let survivors = tree_of(&table, &seen)
        .into_iter()
        .filter(|pid| !table.iter().any(|(p, _, z)| p == pid && *z))
        .count();
    let _ = child.try_wait();
    if survivors == 0 {
        return Ok(());
    }
    Err(harness(format!(
        "{survivors} process(es) of the interrupted command survived SIGKILL (of {left} signalled)"
    )))
}

/// The children whose group SIGKILL left non-empty (`Proc`'s drop), for `quiesce`. Each is kept
/// as its `Child`, whose pid is its group's id, so that a leader not yet reaped still can be.
static SURVIVORS: Mutex<Vec<(String, Child)>> = Mutex::new(Vec::new());

/// Tests that start children share `SURVIVORS`, which the quiescence test also empties and
/// checks: they take this lock, so none can add to it while another asserts on it.
#[cfg(test)]
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Quiescence, before teardown touches a credential: every process group the harness started
/// is empty. Each group its handle's drop left non-empty is probed again for up to 10 s; the
/// error names each that still holds a process.
pub fn quiesce() -> Result<(), HarnessError> {
    quiesce_within(Duration::from_secs(10))
}

fn quiesce_within(patience: Duration) -> Result<(), HarnessError> {
    let mut left = SURVIVORS.lock().unwrap_or_else(PoisonError::into_inner);
    settle(patience, || left.iter_mut().all(|(_, c)| done(c)));
    left.retain_mut(|(_, c)| !done(c));
    if left.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = left
        .iter()
        .map(|(what, c)| format!("{what} (process group {})", c.id()))
        .collect();
    Err(harness(format!(
        "a process group of the harness still holds a process after SIGKILL: {}",
        names.join(", ")
    )))
}

/// A child the harness started, the leader of its own process group. When it is dropped, the
/// child is reaped and the group ended (`end_group`): a child still running is a handle given
/// up early, and a descendant left behind by one that exited is no less a process of the
/// harness. A group the kill leaves non-empty is kept for `quiesce`.
struct Proc {
    what: String,
    child: Option<Child>,
}

impl Proc {
    fn child(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("a Proc holds its child until it is dropped")
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().map_or(0, Child::id)
    }

    fn exited(&mut self) -> bool {
        reaped(self.child())
    }

    fn status(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child().try_wait()
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if !end_group(&mut child) {
                SURVIVORS
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((std::mem::take(&mut self.what), child));
            }
        }
    }
}

/// A process started in the background.
pub struct Running {
    proc: Proc,
    redact: Redactor,
    cancel: Cancel,
    out: Receiver<Vec<u8>>,
    err: Receiver<Vec<u8>>,
    started: Instant,
    timeout: Duration,
}

impl Running {
    pub fn pid(&self) -> u32 {
        self.proc.pid()
    }

    pub fn finished(&mut self) -> bool {
        self.proc.exited()
    }

    /// Waits until it exits or its deadline passes; then its whole process group is sent
    /// SIGTERM, and SIGKILL 2 s later. A cancellation point: a signal ends the group the same
    /// way and returns `interrupted`.
    pub fn wait(mut self) -> Result<Ran, HarnessError> {
        let deadline = self.started + self.timeout;
        let mut timed_out = false;
        let status = loop {
            if let Some(s) = self.proc.status()? {
                break Some(s);
            }
            if let Some(n) = self.cancel.requested() {
                end_group(self.proc.child());
                return Err(interrupted(n));
            }
            if Instant::now() >= deadline {
                timed_out = true;
                end_group(self.proc.child());
                break self.proc.status()?;
            }
            thread::sleep(Duration::from_millis(20));
        };
        let grace = Duration::from_secs(2);
        // Redacted as captured, before anything formats them.
        let (out, err) = (
            self.out.recv_timeout(grace).unwrap_or_default(),
            self.err.recv_timeout(grace).unwrap_or_default(),
        );
        Ok(Ran {
            what: self.proc.what.clone(),
            code: status.and_then(|s| s.code()),
            stdout: self.redact.output(&out),
            stderr: self.redact.output(&err),
            seconds: self.started.elapsed().as_secs_f64(),
            timed_out,
            raw: Raw(out),
        })
    }
}

/// A process in a pseudo-terminal of its own (`script`), for `claude`'s interactive sessions.
/// What it prints is kept as a transcript.
pub struct Pty {
    proc: Proc,
    redact: Redactor,
    cancel: Cancel,
    input: ChildStdin,
    transcript: Arc<Mutex<Vec<u8>>>,
}

impl Pty {
    /// Types `text`, then presses Enter half a second later, as a person would. A signal in
    /// that half second stops it before Enter.
    pub fn line(&mut self, text: &str) -> io::Result<()> {
        self.input.write_all(text.as_bytes())?;
        self.input.flush()?;
        poll(Duration::from_millis(500), Some(&self.cancel), || false);
        if let Some(n) = self.cancel.requested() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, interrupted(n).0));
        }
        self.input.write_all(b"\r")?;
        self.input.flush()
    }

    /// The end of what it printed, redacted whole before it is cut.
    pub fn transcript(&self, n: usize) -> String {
        let text = strip_ansi(&self.transcript.lock().unwrap());
        tail(&self.redact.text(&text), n)
    }

    pub fn finished(&mut self) -> bool {
        self.proc.exited()
    }

    /// Waits for it to exit; after `timeout`, or at a signal, ends its process group as
    /// `Running::wait` does.
    pub fn finish(mut self, timeout: Duration) -> Result<Option<i32>, HarnessError> {
        if !poll(timeout, Some(&self.cancel), || self.proc.exited()) {
            end_group(self.proc.child());
        }
        if let Some(n) = self.cancel.requested() {
            return Err(interrupted(n));
        }
        Ok(self.proc.status()?.and_then(|s| s.code()))
    }
}

/// A `claude` or `tagteam` to start.
#[derive(Clone)]
pub struct Cmd {
    program: PathBuf,
    args: Vec<OsString>,
    vars: Vec<(OsString, OsString)>,
    cwd: PathBuf,
    timeout: Duration,
    stdin: Option<Vec<u8>>,
    redact: Redactor,
    cancel: Cancel,
}

/// What a `{:?}` shows: the program, its arguments as the redactor shows them, and where and
/// for how long it runs. Never `vars`, which may hold a credential, or `stdin`, which does.
impl fmt::Debug for Cmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let args: Vec<String> = self
            .args
            .iter()
            .map(|a| self.redact.text(&a.to_string_lossy()))
            .collect();
        f.debug_struct("Cmd")
            .field("program", &self.program)
            .field("args", &args)
            .field("cwd", &self.cwd)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Cmd {
    /// `vars` is the whole environment.
    pub fn new(program: &Path, vars: Vec<(OsString, OsString)>, cwd: &Path) -> Self {
        Self {
            program: program.to_path_buf(),
            args: Vec::new(),
            vars,
            cwd: cwd.to_path_buf(),
            timeout: Duration::from_secs(60),
            stdin: None,
            redact: Redactor::default(),
            cancel: cancel().clone(),
        }
    }

    pub fn args<S: AsRef<OsStr>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    /// Sets `name`, replacing any value it had.
    pub fn var(mut self, name: &str, value: impl AsRef<OsStr>) -> Self {
        self.vars.retain(|(n, _)| n != name);
        self.vars.push((name.into(), value.as_ref().to_owned()));
        self
    }

    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }

    /// What its output is redacted with as it is captured; without it, token-shaped runs only.
    pub fn redact(mut self, redactor: &Redactor) -> Self {
        self.redact = redactor.clone();
        self
    }

    /// The token it stops at: the process's (`cancel()`) unless a test gives its own.
    pub fn cancel(mut self, token: &Cancel) -> Self {
        self.cancel = token.clone();
        self
    }

    /// A cancellation point: once a signal is recorded, nothing new starts.
    fn not_cancelled(&self) -> Result<(), HarnessError> {
        match self.cancel.requested() {
            Some(n) => Err(interrupted(n)),
            None => Ok(()),
        }
    }

    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }

    pub fn describe(&self) -> String {
        let name = self
            .program
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        std::iter::once(name)
            .chain(self.args.iter().map(|a| a.to_string_lossy().into_owned()))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn command(&self, program: &Path, args: &[OsString]) -> Command {
        let mut c = Command::new(program);
        c.args(args)
            .env_clear()
            .envs(self.vars.iter().map(|(k, v)| (k, v)))
            .current_dir(&self.cwd);
        c
    }

    /// Starts it in the background, in a process group of its own, output captured.
    pub fn spawn(self, roots: &Roots) -> Result<Running, HarnessError> {
        self.not_cancelled()?;
        roots.check_env(&self.vars)?;
        let mut c = self.command(&self.program, &self.args);
        c.process_group(0)
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c
            .spawn()
            .map_err(|e| harness(format!("could not start {}: {e}", self.describe())))?;
        if let (Some(bytes), Some(mut pipe)) = (self.stdin.clone(), child.stdin.take()) {
            thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
            });
        }
        Ok(Running {
            out: drain(child.stdout.take()),
            err: drain(child.stderr.take()),
            proc: Proc {
                what: self.redact.text(&self.describe()),
                child: Some(child),
            },
            started: Instant::now(),
            timeout: self.timeout,
            redact: self.redact,
            cancel: self.cancel,
        })
    }

    pub fn run(self, roots: &Roots) -> Result<Ran, HarnessError> {
        self.spawn(roots)?.wait()
    }

    /// With the terminal attached and no deadline, for the steps a person answers (`compat
    /// login`). It stays in the terminal's process group, so Ctrl-C reaches it. A cancellation
    /// point (`wait_interactive`): a signal recorded in the token ends it and returns
    /// `interrupted`, so the run unwinds instead of waiting on the person.
    pub fn attached(self, roots: &Roots) -> Result<Option<i32>, HarnessError> {
        self.not_cancelled()?;
        roots.check_env(&self.vars)?;
        let mut child = spawn_interactive(&mut self.command(&self.program, &self.args))
            .map_err(|e| harness(format!("could not start {}: {e}", self.describe())))?;
        Ok(wait_interactive(&mut child, &self.cancel)?.code())
    }

    /// In a pseudo-terminal of 120 by 40 that `script` makes, so `claude` runs its interactive
    /// interface; what is written to the returned `Pty` is typed into it.
    pub fn pty(self, roots: &Roots) -> Result<Pty, HarnessError> {
        self.not_cancelled()?;
        roots.check_env(&self.vars)?;
        let mut inner: Vec<OsString> = vec![
            "/bin/sh".into(),
            "-c".into(),
            "stty rows 40 cols 120 2>/dev/null; exec \"$@\"".into(),
            "sh".into(),
            self.program.clone().into_os_string(),
        ];
        inner.extend(self.args.iter().cloned());
        let args: Vec<OsString> = if cfg!(target_os = "macos") {
            ["-q", "/dev/null"]
                .into_iter()
                .map(OsString::from)
                .chain(inner)
                .collect()
        } else {
            let line = inner
                .iter()
                .map(|a| shell_quote(&a.to_string_lossy()))
                .collect::<Vec<_>>()
                .join(" ");
            vec![
                "-q".into(),
                "-e".into(),
                "-c".into(),
                line.into(),
                "/dev/null".into(),
            ]
        };
        let mut c = self.command(Path::new("/usr/bin/script"), &args);
        c.process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = c.spawn().map_err(|e| {
            harness(format!(
                "could not start script for {}: {e}",
                self.describe()
            ))
        })?;
        let input = child.stdin.take().expect("stdin is piped");
        let transcript = Arc::new(Mutex::new(Vec::new()));
        if let Some(mut out) = child.stdout.take() {
            let sink = transcript.clone();
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = out.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    sink.lock().unwrap().extend_from_slice(&buf[..n]);
                }
            });
        }
        Ok(Pty {
            proc: Proc {
                what: self.redact.text(&self.describe()),
                child: Some(child),
            },
            redact: self.redact,
            cancel: self.cancel,
            input,
            transcript,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_wait_ends_an_interactive_child_and_reaps_it() {
        let _serial = serial();
        let mut child = spawn_interactive(Command::new("/bin/sleep").arg("30")).unwrap();
        let pid = child.id();
        let token = Cancel::new();
        let later = token.clone();
        let signaller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            later.request(SIGTERM);
        });
        let t = Instant::now();
        let err = wait_interactive(&mut child, &token).unwrap_err();
        signaller.join().unwrap();
        assert_eq!(err, interrupted(SIGTERM));
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
        assert!(reaped(&mut child), "the child was reaped");
        assert!(!process_alive(pid));
    }

    #[test]
    fn a_cancelled_attached_command_is_ended_and_reaped() {
        let _serial = serial();
        let dir = std::env::temp_dir().join(format!("xtask-attached-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pidfile = dir.join("pid");
        let token = Cancel::new();
        let later = token.clone();
        let signaller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            later.request(SIGTERM);
        });
        let t = Instant::now();
        let err = sh(&format!("echo $$ > '{}'; exec sleep 30", pidfile.display()))
            .cancel(&token)
            .attached(&test_roots())
            .unwrap_err();
        signaller.join().unwrap();
        assert_eq!(err, interrupted(SIGTERM));
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        let pid: u32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(!process_alive(pid), "the child was ended and reaped");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_cancelled_interactive_wait_ends_the_whole_tree() {
        let _serial = serial();
        let dir = std::env::temp_dir().join(format!("xtask-tree-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = format!(
            "sleep 30 & echo $! > '{d}/one'; sleep 31 & echo $! > '{d}/two'; wait",
            d = dir.display()
        );
        let mut child = Command::new("/bin/sh")
            .args(["-c", &script])
            .spawn()
            .unwrap();
        let token = Cancel::new();
        let later = token.clone();
        let signaller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            later.request(SIGTERM);
        });
        let t = Instant::now();
        let err = wait_interactive(&mut child, &token).unwrap_err();
        signaller.join().unwrap();
        assert_eq!(err, interrupted(SIGTERM));
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        assert!(reaped(&mut child));
        for name in ["one", "two"] {
            let pid: u32 = std::fs::read_to_string(dir.join(name))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert!(!process_alive(pid), "{name} ({pid}) outlived the cancel");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Runs `script` as an interactive child to its end, and returns how long `wait_interactive`
    /// took and the pid its `$!` was recorded as.
    fn leader_that_leaves_one_behind(
        script: &str,
    ) -> (Duration, u32, Result<ExitStatus, HarnessError>) {
        let dir = std::env::temp_dir().join(format!(
            "xtask-leader-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let script = script.replace("PIDFILE", &dir.join("pid").display().to_string());
        let mut child = spawn_interactive(Command::new("/bin/sh").args(["-c", &script])).unwrap();
        let t = Instant::now();
        let result = wait_interactive(&mut child, &Cancel::new());
        let took = t.elapsed();
        let pid: u32 = std::fs::read_to_string(dir.join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        (took, pid, result)
    }

    #[test]
    fn a_leader_that_exits_leaves_no_descendant_that_ignores_sigint() {
        // Codex slice 8 re-review: the leader is gone before the next poll, so a tree walk from
        // it finds nothing; the group is ended on the normal path too.
        let _serial = serial();
        let (took, pid, result) =
            leader_that_leaves_one_behind("trap '' INT; sleep 30 & echo $! > 'PIDFILE'; exit 0");
        assert!(result.unwrap().success());
        assert!(took < Duration::from_secs(3), "{took:?}");
        assert!(
            !process_alive(pid),
            "the descendant ({pid}) outlived the wait"
        );
    }

    #[test]
    fn a_descendant_that_ignores_sigterm_too_is_killed_after_the_grace() {
        let _serial = serial();
        let (took, pid, result) = leader_that_leaves_one_behind(
            "trap '' INT TERM; sleep 30 & echo $! > 'PIDFILE'; exit 0",
        );
        assert!(result.unwrap().success());
        assert!(took < Duration::from_secs(5), "{took:?}");
        assert!(
            !process_alive(pid),
            "the descendant ({pid}) outlived the wait"
        );
    }

    #[test]
    fn a_leader_ended_by_sigint_counts_as_a_cancellation() {
        let _serial = serial();
        let mut child =
            spawn_interactive(Command::new("/bin/sh").args(["-c", "kill -INT $$; sleep 5"]))
                .unwrap();
        let token = Cancel::new();
        let err = wait_interactive(&mut child, &token).unwrap_err();
        assert_eq!(err, interrupted(SIGINT));
        // The run's token records it, as the harness's own handler would, so the callers stop.
        assert_eq!(token.requested(), Some(SIGINT));
    }

    #[test]
    fn a_stopped_child_with_no_terminal_is_a_cancellation_not_a_hang() {
        let _serial = serial();
        let mut child =
            spawn_interactive(Command::new("/bin/sh").args(["-c", "kill -STOP $$; exit 0"]))
                .unwrap();
        let token = Cancel::new();
        let t = Instant::now();
        let err = wait_interactive(&mut child, &token).unwrap_err();
        assert_eq!(err, interrupted(SIGTSTP));
        assert_eq!(token.requested(), Some(SIGTSTP));
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
        assert!(reaped(&mut child));
    }

    #[test]
    fn a_stopped_child_that_holds_the_terminal_is_resumed() {
        let _serial = serial();
        let dir = std::env::temp_dir().join(format!("xtask-resume-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let done = dir.join("done");
        let script = format!("kill -STOP $$; echo ok > '{}'", done.display());
        let mut child = spawn_interactive(Command::new("/bin/sh").args(["-c", &script])).unwrap();
        let status = wait_interactive_as(&mut child, &Cancel::new(), true).unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read_to_string(&done).unwrap().trim(), "ok");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_child_that_does_not_lead_its_own_group_is_never_signalled_as_one() {
        // Started without `spawn_interactive`, it shares the harness's group: a group signal
        // would hit the test process. It is waited for and swept as a tree instead.
        let _serial = serial();
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 3"])
            .spawn()
            .unwrap();
        assert!(!leads_its_group(&child));
        let status = wait_interactive(&mut child, &Cancel::new()).unwrap();
        assert_eq!(status.code(), Some(3));
    }

    #[test]
    fn a_process_listing_is_parsed_into_a_tree() {
        let table = parse_process_table(
            "  1     0 Ss\n 10     1 S\n 11    10 S+\n 12    10 Z\n 99    98 S\n",
        );
        let tree = tree_of(&table, &BTreeSet::from([10]));
        assert_eq!(tree, BTreeSet::from([10, 11, 12]));
        assert!(table.contains(&(12, 10, true)));
        assert!(tree_of(&table, &BTreeSet::from([4242])).is_empty());
    }

    #[test]
    fn an_interactive_child_that_exits_gives_its_status() {
        let _serial = serial();
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 90"])
            .spawn()
            .unwrap();
        let status = wait_interactive(&mut child, &Cancel::new()).unwrap();
        assert_eq!(status.code(), Some(90));
    }

    #[test]
    fn shell_quoting_survives_quotes() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn a_transcript_loses_its_escapes_and_keeps_its_text() {
        let raw = b"\x1b[2J\x1b[1;1H\x1b[38;5;12mok\x1b[0m\r\n\x1b]0;title\x07next\rline\x08";
        assert_eq!(strip_ansi(raw), "ok\nnext\nline");
    }

    #[test]
    fn which_finds_the_first_executable_on_the_path() {
        let found = which_in("sh", OsStr::new("/nonexistent:/bin")).unwrap();
        assert_eq!(found, PathBuf::from("/bin/sh"));
        assert!(which_in("no-such-tool-anywhere", OsStr::new("/bin")).is_none());
    }

    #[test]
    fn the_end_of_a_long_text_is_kept() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("ab", 3), "ab");
    }

    #[test]
    fn a_spawn_needs_the_guard_s_consent() {
        let _serial = serial();
        let roots = Roots {
            scratch: PathBuf::from("/tmp/tagteam-compat.test"),
            state: PathBuf::from("/nonexistent/state"),
            users: vec![],
            home: std::env::temp_dir(),
            user: None,
        };
        let refused = Cmd::new(Path::new("/bin/sh"), vec![], Path::new("/")).run(&roots);
        assert!(refused.unwrap_err().0.contains("CLAUDE_CONFIG_DIR"));
        let ran = Cmd::new(
            Path::new("/bin/sh"),
            vec![(
                "CLAUDE_CONFIG_DIR".into(),
                "/tmp/tagteam-compat.test/live".into(),
            )],
            Path::new("/"),
        )
        .args(["-c", "echo \"$CLAUDE_CONFIG_DIR\"; echo err >&2; exit 3"])
        .run(&roots)
        .unwrap();
        assert_eq!(ran.code, Some(3));
        assert_eq!(ran.stdout_text(), "/tmp/tagteam-compat.test/live\n");
        assert_eq!(ran.summary()["stderr"], "err\n");
    }

    #[test]
    fn a_process_past_its_deadline_is_ended_with_its_group() {
        let _serial = serial();
        let roots = Roots {
            scratch: PathBuf::from("/tmp/tagteam-compat.test"),
            state: PathBuf::from("/nonexistent/state"),
            users: vec![],
            home: std::env::temp_dir(),
            user: None,
        };
        let ran = Cmd::new(
            Path::new("/bin/sh"),
            vec![(
                "CLAUDE_CONFIG_DIR".into(),
                "/tmp/tagteam-compat.test/live".into(),
            )],
            Path::new("/"),
        )
        .args(["-c", "sleep 30 & wait"])
        .timeout(Duration::from_millis(300))
        .run(&roots)
        .unwrap();
        assert!(ran.timed_out);
        assert!(ran.seconds < 5.0, "{}", ran.seconds);
    }

    fn test_roots() -> Roots {
        Roots {
            scratch: PathBuf::from("/tmp/tagteam-compat.test"),
            state: PathBuf::from("/nonexistent/state"),
            users: vec![],
            home: std::env::temp_dir(),
            user: None,
        }
    }

    fn sh(script: &str) -> Cmd {
        Cmd::new(
            Path::new("/bin/sh"),
            vec![(
                "CLAUDE_CONFIG_DIR".into(),
                "/tmp/tagteam-compat.test/live".into(),
            )],
            Path::new("/"),
        )
        .args(["-c", script])
    }

    #[test]
    fn a_running_dropped_early_leaves_no_live_child() {
        let _serial = serial();
        let running = sh("sleep 30 & wait").spawn(&test_roots()).unwrap();
        let pid = running.pid();
        assert!(group_alive(pid), "its group runs");
        drop(running);
        assert!(!signal(pid, "0", false), "the child was reaped");
        assert!(!group_alive(pid), "nothing of its group runs");
    }

    #[test]
    fn a_descendant_that_ignores_sigterm_is_killed_with_its_group() {
        let _serial = serial();
        // The leader ends on SIGTERM; its child, and the child's `sleep`, ignore it.
        let ready = std::env::temp_dir().join(format!("xtask-sys-trap-{}", std::process::id()));
        let _ = std::fs::remove_file(&ready);
        let running = sh(&format!(
            "sh -c 'trap \"\" TERM; touch {}; while :; do sleep 1; done' & wait",
            shell_quote(&ready.to_string_lossy())
        ))
        .spawn(&test_roots())
        .unwrap();
        let pgid = running.pid();
        assert!(wait_until(Duration::from_secs(5), || ready.exists()));
        drop(running);
        assert!(!group_alive(pgid), "SIGKILL ended what SIGTERM left");
        std::fs::remove_file(&ready).unwrap();
    }

    #[test]
    fn a_child_s_output_is_redacted_as_it_is_captured() {
        let _serial = serial();
        use crate::compat::report::{CheckResult, Evidence, Outcome, Report, Status};
        let org = r#"Acme "Research""#;
        let mut redact = Redactor::default();
        redact.learn(org, "<account 1 org>".into());
        let ran = sh(r#"echo 'organization Acme "Research" refused' >&2
printf '%s' '{"organizationName":"Acme \"Research\""}'
exit 1"#)
        .redact(&redact)
        .run(&test_roots())
        .unwrap();
        // The report's own pass is left empty: only the capture can have redacted it.
        let report = Report {
            harness_error: Some(format!(
                "tagteam list failed: {} with {}",
                ran.summary(),
                ran.stdout_text()
            )),
            checks: vec![CheckResult {
                id: "auth-status",
                title: "a check",
                outcome: Outcome {
                    status: Status::Fail,
                    summary: format!("not as expected: {}", ran.summary()),
                    evidence: vec![Evidence {
                        label: "tagteam list".into(),
                        value: json!({"summary": ran.summary().to_string(), "out": ran.stdout_text()}),
                        ok: Some(false),
                    }],
                },
                seconds: 1.0,
            }],
            ..Report::default()
        };
        let dir = std::env::temp_dir().join(format!("xtask-sys-report-{}", std::process::id()));
        report.write(&dir).unwrap();
        for name in ["report.json", "report.md"] {
            let written = std::fs::read_to_string(dir.join(name)).unwrap();
            assert!(!written.contains("Research"), "{name}: {written}");
            assert!(written.contains("<account 1 org>"), "{name}: {written}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_cancelled_wait_ends_the_group_and_nothing_new_starts() {
        let _serial = serial();
        let token = Cancel::new();
        let running = sh("sleep 30 & wait")
            .cancel(&token)
            .spawn(&test_roots())
            .unwrap();
        let pgid = running.pid();
        let signaller = token.clone();
        let raised = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            signaller.request(SIGINT);
        });
        let started = Instant::now();
        assert_eq!(running.wait().unwrap_err().0, "interrupted by SIGINT");
        raised.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!signal(pgid, "0", false), "the child was reaped");
        assert!(!group_alive(pgid), "and its group is empty");
        let refused = sh("exit 0").cancel(&token).run(&test_roots()).unwrap_err();
        assert_eq!(refused.0, "interrupted by SIGINT");
    }

    #[test]
    fn a_signal_sets_the_cancel_token() {
        let token = Cancel::new();
        // SIGTERM alone: this test process's own SIGINT stays the terminal's.
        catch_signals(&token, &[SIGTERM]).unwrap();
        signal_hook::low_level::raise(SIGTERM).unwrap();
        assert!(wait_until(Duration::from_secs(1), || token
            .requested()
            .is_some()));
        assert_eq!(token.requested(), Some(15));
        assert_eq!(CAUGHT, [SIGINT, SIGTERM, SIGHUP]);
        assert_eq!(signal_name(1), "SIGHUP");
    }

    #[test]
    fn quiescence_waits_for_every_recorded_group_and_names_one_that_stays() {
        let _serial = serial();
        let child = Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = child.id();
        SURVIVORS.lock().unwrap().push(("sleep 30".into(), child));
        let refused = quiesce_within(Duration::from_millis(300)).unwrap_err();
        assert!(
            refused
                .0
                .contains(&format!("sleep 30 (process group {pgid})")),
            "{refused}"
        );
        signal(pgid, "KILL", true);
        assert_eq!(quiesce_within(Duration::from_secs(5)), Ok(()));
        assert!(!group_alive(pgid), "its leader was reaped");
        assert!(SURVIVORS.lock().unwrap().is_empty());
    }

    #[test]
    fn a_command_s_debug_output_omits_its_environment_and_standard_input() {
        let cmd = Cmd::new(
            Path::new("/bin/sh"),
            vec![(
                "CLAUDE_CODE_OAUTH_TOKEN".into(),
                "sk-ant-oat01-secret".into(),
            )],
            Path::new("/"),
        )
        .args(["-c", "true"])
        .stdin(b"sk-ant-api03-secret".to_vec());
        let shown = format!("{cmd:?}");
        assert!(
            !shown.contains("secret") && !shown.contains("CLAUDE_CODE"),
            "{shown}"
        );
        assert!(
            shown.contains("/bin/sh") && shown.contains("\"-c\""),
            "{shown}"
        );
    }
}
