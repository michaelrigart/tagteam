//! §12.6: session records and whether the process that wrote one still lives. Reservations are
//! judged by their lock alone (§12.5, `flock::probe_lock`); this is the record side.

#[cfg(any(test, feature = "test-doubles"))]
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "test-doubles"))]
use std::sync::{Mutex, PoisonError};

use crate::read::{Read, ReadError};

/// When a recorded process started, in the form its record gives it (§12.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcStart {
    /// Epoch seconds.
    EpochS(i64),
    /// Linux boot ticks, `/proc/<pid>/stat` field 22.
    BootTicks(u64),
    /// Boot ticks too large for a `u64`.
    BootTicksIndeterminate,
    /// None given, or none this build can read.
    Unknown,
}

/// A session record, reduced to what liveness and the token report's sightings need (§12.6,
/// token report spec §2.4). A provider's `parse_session_record` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub pid: u32,
    pub start: ProcStart,
    /// When the session's execution started, epoch ms.
    pub started_at_ms: Option<i64>,
    pub session_id: Option<String>,
    /// Written by a background session rather than an interactive one.
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordEntry<R = SessionRecord> {
    Record(R),
    /// Present but not a readable record (§12.6: it counts as unreadable). `detail` never
    /// quotes the file's bytes.
    Unreadable {
        path: PathBuf,
        detail: String,
    },
}

/// Every `<dir>/*.<ext>` in name order, each parsed by `parse` and dated by `clock` right after
/// its read, before it is parsed and before the next read: what dates a sighting (token report
/// spec §6.4). `clock` is asked once for each file read. A missing `dir` is `Present(vec![])`,
/// and so is one that is not a directory, or under a profile path that is not one (`ENOTDIR`):
/// no record can be written there. A `dir` that cannot be listed is `Unreadable`. A record that
/// vanishes between the listing and its read has been removed by its session's exit, so it is
/// skipped.
pub fn read_session_records<R>(
    dir: &Path,
    ext: &str,
    parse: &dyn Fn(&[u8]) -> Result<R, String>,
    clock: &mut dyn FnMut() -> i64,
) -> Read<Vec<(RecordEntry<R>, i64)>> {
    let unreadable =
        |e: io::Error| Read::Unreadable(ReadError::new(dir.display().to_string(), e.to_string()));
    // Nothing that is not a directory, the profile's own path included, holds a record.
    let listing = match fs::read_dir(dir) {
        Ok(l) => l,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Read::Present(vec![]);
        }
        Err(e) => return unreadable(e),
    };
    let mut paths = Vec::new();
    for entry in listing {
        match entry {
            Ok(e) => {
                let path = e.path();
                if path.extension().is_some_and(|x| x == ext) {
                    paths.push(path);
                }
            }
            // A listing that fails part-way may hide a record.
            Err(e) => return unreadable(e),
        }
    }
    paths.sort();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let read = fs::read(&path);
        let at = clock();
        let entry = match read {
            Ok(bytes) => match parse(&bytes) {
                Ok(r) => RecordEntry::Record(r),
                Err(detail) => RecordEntry::Unreadable { path, detail },
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => RecordEntry::Unreadable {
                path,
                detail: e.to_string(),
            },
        };
        out.push((entry, at));
    }
    Read::Present(out)
}

/// `<dir>/<pid>.<ext>`.
pub fn session_record_path(dir: &Path, pid: u32, ext: &str) -> PathBuf {
    dir.join(format!("{pid}.{ext}"))
}

/// A background supervisor's lock file (CC 2.1.292: `<profile>/daemon.lock`, JSON with at least
/// `pid`, `origin` and `procStart`), read with `parse` as a session record is (§12.6): the
/// supervisor's `pid` and start are what liveness needs. A missing file, or one under a profile
/// path that is not a directory, is `Absent`; one that cannot be read or parsed is
/// `Unreadable`, and counts as owned like an unreadable record. `detail` never quotes the
/// file's bytes.
pub fn read_supervisor_lock<R>(path: &Path, parse: &dyn Fn(&[u8]) -> Result<R, String>) -> Read<R> {
    let unreadable =
        |detail: String| Read::Unreadable(ReadError::new(path.display().to_string(), detail));
    match fs::read(path) {
        Ok(bytes) => match parse(&bytes) {
            Ok(r) => Read::Present(r),
            Err(detail) => unreadable(detail),
        },
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Read::Absent
        }
        Err(e) => unreadable(e.to_string()),
    }
}

/// §4.2's process port: the OS facts §12.6 judges a session record by. Every answer is
/// `None` when it cannot be determined, which `record_is_live` counts as live.
pub trait ProcessProbe: Send + Sync {
    /// `kill(pid, 0)`: `Some(true)` on success or `EPERM`, `Some(false)` on `ESRCH`, `None` otherwise.
    fn exists(&self, pid: u32) -> Option<bool>;
    /// The process's start time in epoch seconds.
    fn start_time_s(&self, pid: u32) -> Option<i64>;
    /// Linux `/proc/<pid>/stat` field 22, for the all-digit legacy `procStart`.
    fn start_ticks(&self, pid: u32) -> Option<u64>;
    /// Whether the executable name or the arguments contain `needle`.
    fn mentions(&self, pid: u32, needle: &str) -> Option<bool>;
    /// The parent's pid, for the statusline's walk to its session (token report spec §6.3,
    /// ruling R3). `None` when it cannot be determined, or the process has no parent.
    fn parent_pid(&self, _pid: u32) -> Option<u32> {
        None
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// The real OS: `kill(2)`, then `proc_pidinfo` and `sysctl(KERN_PROCARGS2)` on macOS, or
/// `/proc` on Linux.
pub struct SystemProcessProbe;

impl ProcessProbe for SystemProcessProbe {
    fn exists(&self, pid: u32) -> Option<bool> {
        let pid = libc::pid_t::try_from(pid).ok().filter(|p| *p > 0)?;
        // SAFETY: signal 0 sends nothing; `kill` only checks that `pid` exists and may be
        // signalled. `pid` is positive, so it names one process, never a group.
        let rc = unsafe { libc::kill(pid, 0) };
        if rc == 0 {
            return Some(true);
        }
        match io::Error::last_os_error().raw_os_error() {
            Some(libc::EPERM) => Some(true),
            Some(libc::ESRCH) => Some(false),
            _ => None,
        }
    }

    #[cfg(target_os = "macos")]
    fn start_time_s(&self, pid: u32) -> Option<i64> {
        let micros = crate::process::start_of(pid).ok()??;
        i64::try_from(micros / 1_000_000).ok()
    }

    #[cfg(target_os = "linux")]
    fn start_time_s(&self, pid: u32) -> Option<i64> {
        let ticks = crate::process::start_of(pid).ok()??;
        let hz = clock_ticks_per_s()?;
        Some(boot_time_s()? + i64::try_from(ticks / hz).ok()?)
    }

    #[cfg(target_os = "macos")]
    fn start_ticks(&self, _pid: u32) -> Option<u64> {
        None
    }

    #[cfg(target_os = "linux")]
    fn start_ticks(&self, pid: u32) -> Option<u64> {
        crate::process::start_of(pid).ok()?
    }

    #[cfg(target_os = "macos")]
    fn mentions(&self, pid: u32, needle: &str) -> Option<bool> {
        let words = process_words(pid)?;
        Some(words.iter().any(|w| contains(w, needle.as_bytes())))
    }

    #[cfg(target_os = "linux")]
    fn mentions(&self, pid: u32, needle: &str) -> Option<bool> {
        let needle = needle.as_bytes();
        let comm = fs::read(format!("/proc/{pid}/comm")).ok();
        let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok();
        match (comm, cmdline) {
            (Some(c), Some(a)) => Some(contains(&c, needle) || contains(&a, needle)),
            (Some(one), None) | (None, Some(one)) if contains(&one, needle) => Some(true),
            _ => None,
        }
    }

    fn parent_pid(&self, pid: u32) -> Option<u32> {
        crate::process::parent_of(pid).ok()?
    }
}

/// `btime` in `/proc/stat`: the boot time, epoch seconds.
#[cfg(target_os = "linux")]
fn boot_time_s() -> Option<i64> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    stat.lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()
}

/// `sysconf(_SC_CLK_TCK)`: the unit of `/proc/<pid>/stat` field 22.
#[cfg(target_os = "linux")]
fn clock_ticks_per_s() -> Option<u64> {
    // SAFETY: `sysconf` only reads a configuration value.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(hz).ok().filter(|h| *h > 0)
}

/// The executable path and the arguments, from `sysctl(KERN_PROCARGS2)`. `None` when they
/// cannot be read: another user's process, a zombie, or a pid that is gone.
#[cfg(target_os = "macos")]
fn process_words(pid: u32) -> Option<Vec<Vec<u8>>> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `mib` names a two-level integer sysctl, `argmax` is a writable `c_int` and
    // `size` holds its size; nothing is written back to the kernel.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut argmax).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    let len = usize::try_from(argmax).ok().filter(|n| rc == 0 && *n > 0)?;
    let mut buf = vec![0u8; len];
    let mut size = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: `buf` is writable for `size` bytes; `sysctl` writes at most that many and
    // stores the count it wrote in `size`.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    parse_procargs(&buf[..size.min(len)])
}

/// Splits a `KERN_PROCARGS2` buffer: a native-endian `int` argc, the executable path, NUL
/// padding, then `argc` NUL-terminated arguments. The environment after them is never read: a
/// variable such as `CLAUDE_CONFIG_DIR` must not count as mentioning the launch command.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_procargs(buf: &[u8]) -> Option<Vec<Vec<u8>>> {
    let n = std::mem::size_of::<i32>();
    let argc = i32::from_ne_bytes(buf.get(..n)?.try_into().ok()?);
    let argc = usize::try_from(argc).ok()?;
    let rest = &buf[n..];
    let end = rest.iter().position(|b| *b == 0)?;
    let mut words = vec![rest[..end].to_vec()];
    let mut rest = &rest[end..];
    let start = rest.iter().position(|b| *b != 0).unwrap_or(rest.len());
    rest = &rest[start..];
    for _ in 0..argc {
        if rest.is_empty() {
            break;
        }
        let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
        words.push(rest[..end].to_vec());
        rest = rest.get(end + 1..).unwrap_or(&[]);
    }
    Some(words)
}

/// One process as `FakeProcessProbe` answers for it. Every field defaults to `None`, "cannot be
/// determined"; `exists: None` therefore makes a record live whatever else is set.
#[cfg(any(test, feature = "test-doubles"))]
#[derive(Debug, Clone, Default)]
pub struct FakeProcess {
    pub exists: Option<bool>,
    pub start_time_s: Option<i64>,
    pub start_ticks: Option<u64>,
    /// `mentions`' answer, whatever the needle.
    pub mentions_launch: Option<bool>,
    /// `parent_pid`'s answer.
    pub ppid: Option<u32>,
}

/// Tests only: behind the `test-doubles` feature. Unknown pids are `exists: Some(false)`.
#[cfg(any(test, feature = "test-doubles"))]
#[derive(Default)]
pub struct FakeProcessProbe {
    procs: Mutex<BTreeMap<u32, FakeProcess>>,
}

#[cfg(any(test, feature = "test-doubles"))]
impl FakeProcessProbe {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, pid: u32, p: FakeProcess) {
        self.procs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(pid, p);
    }

    fn get(&self, pid: u32) -> Option<FakeProcess> {
        self.procs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&pid)
            .cloned()
    }
}

#[cfg(any(test, feature = "test-doubles"))]
impl ProcessProbe for FakeProcessProbe {
    fn exists(&self, pid: u32) -> Option<bool> {
        match self.get(pid) {
            Some(p) => p.exists,
            None => Some(false),
        }
    }

    fn start_time_s(&self, pid: u32) -> Option<i64> {
        self.get(pid)?.start_time_s
    }

    fn start_ticks(&self, pid: u32) -> Option<u64> {
        self.get(pid)?.start_ticks
    }

    fn mentions(&self, pid: u32, _needle: &str) -> Option<bool> {
        self.get(pid)?.mentions_launch
    }

    fn parent_pid(&self, pid: u32) -> Option<u32> {
        self.get(pid)?.ppid
    }
}

/// How long after `startedAt` a process must have started, by cswap's rule, before it can be
/// judged a recycled pid (§12.6).
const RECYCLE_GRACE_MS: i64 = 120_000;

/// §12.6: live if the pid exists and still belongs to the record's writer, judged by the form of
/// its process start. `launch_command` is the provider's (`claude`), used by the heuristic for an
/// unknown start and, under Decision 16, to judge an epoch-seconds mismatch (recycled only if the
/// process does not mention it).
/// Anything that cannot be determined counts as live.
pub fn record_is_live(probe: &dyn ProcessProbe, r: &SessionRecord, launch_command: &str) -> bool {
    match probe.exists(r.pid) {
        Some(false) => return false,
        None => return true,
        Some(true) => {}
    }
    match r.start {
        ProcStart::EpochS(recorded) => {
            // The OS start time equal to the record's within ±1 s, or unknown, is live. A
            // mismatch alone is no proof (Decision 16): a Linux start time moves with the wall
            // clock, so it is recycled only when the process does not mention the launch command
            // either. An unknown answer counts as mentioning it.
            let matches = probe
                .start_time_s(r.pid)
                .is_none_or(|actual| (actual - recorded).abs() <= 1);
            matches || probe.mentions(r.pid, launch_command).unwrap_or(true)
        }
        ProcStart::BootTicks(recorded) => probe
            .start_ticks(r.pid)
            .is_none_or(|actual| actual == recorded),
        // Too large to compare with any start tick, so it cannot show a recycled pid.
        ProcStart::BootTicksIndeterminate => true,
        ProcStart::Unknown => {
            // No usable start: recycled only if the process started more than 120 s after
            // `startedAt` and neither its executable name nor its arguments mention the launch
            // command.
            let started_late = match (r.started_at_ms, probe.start_time_s(r.pid)) {
                // Whole seconds round the start down, so this never calls a process late early.
                (Some(at_ms), Some(start_s)) => {
                    start_s.saturating_mul(1000) > at_ms.saturating_add(RECYCLE_GRACE_MS)
                }
                _ => false,
            };
            !started_late || probe.mentions(r.pid, launch_command) != Some(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A test-local record format: UTF-8 decimal text, a pid above 0.
    fn pid_text(bytes: &[u8]) -> Result<u32, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "not text".to_string())?;
        match text.parse::<u32>() {
            Ok(pid) if pid > 0 => Ok(pid),
            _ => Err("not a pid".to_string()),
        }
    }

    #[test]
    fn records_are_listed_in_name_order_and_a_malformed_one_is_unreadable() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("sessions");
        assert!(
            matches!(
                read_session_records(&dir, "rec", &pid_text, &mut || 0),
                Read::Present(v) if v.is_empty()
            ),
            "a missing directory holds no records"
        );
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("20.rec"), b"20").unwrap();
        fs::write(dir.join("10.rec"), b"10").unwrap();
        fs::write(dir.join("30.rec"), b"{").unwrap();
        fs::write(dir.join("notes.txt"), b"not a record").unwrap();
        fs::write(dir.join("50.json"), b"50").unwrap();
        fs::write(dir.join("keeper.lock"), b"60").unwrap();
        fs::create_dir(dir.join("40.rec")).unwrap();
        let Read::Present(entries) = read_session_records(&dir, "rec", &pid_text, &mut || 0) else {
            panic!("the directory lists");
        };
        let entries: Vec<RecordEntry<u32>> = entries.into_iter().map(|(e, _)| e).collect();
        assert_eq!(
            entries[..2],
            [RecordEntry::Record(10), RecordEntry::Record(20)]
        );
        assert!(
            matches!(&entries[2], RecordEntry::Unreadable { path, .. } if path == &dir.join("30.rec"))
        );
        assert!(
            matches!(&entries[3], RecordEntry::Unreadable { path, .. } if path == &dir.join("40.rec")),
            "a directory named like a record cannot be read"
        );
        assert_eq!(
            entries.len(),
            4,
            "notes.txt, 50.json and keeper.lock are not records"
        );
    }

    #[test]
    fn each_record_is_dated_right_after_its_own_read() {
        // Token report spec §6.4: the scan dates a sighting at the read of its record, not at
        // the end of the directory's. `clock` is asked once for each file, right after its read
        // and before the next one; this one counts its calls.
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("sessions");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("20.rec"), b"20").unwrap();
        fs::write(dir.join("10.rec"), b"10").unwrap();
        fs::write(dir.join("30.rec"), b"{").unwrap();
        fs::write(dir.join("notes.txt"), b"not a record").unwrap();
        let mut calls = 0;
        let mut clock = || {
            calls += 1;
            calls * 1_000
        };
        let Read::Present(entries) = read_session_records(&dir, "rec", &pid_text, &mut clock)
        else {
            panic!("the directory lists");
        };
        let dates: Vec<i64> = entries.iter().map(|(_, at)| *at).collect();
        assert_eq!(dates, [1_000, 2_000, 3_000]);
        assert_eq!(entries[0].0, RecordEntry::Record(10));
        assert_eq!(entries[1].0, RecordEntry::Record(20));
        assert!(matches!(&entries[2].0, RecordEntry::Unreadable { .. }));
        assert_eq!(calls, 3, "only the three record files are dated");
    }

    #[test]
    fn a_records_directory_that_cannot_be_listed_is_unreadable() {
        // Run as a non-root user: root lists a 0o000 directory.
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("sessions");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
        let found = read_session_records(&dir, "rec", &pid_text, &mut || 0);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(found, Read::Unreadable(_)), "{found:?}");
    }

    #[test]
    fn no_record_can_lie_under_something_that_is_not_a_directory() {
        // A regular file at the records directory's path, or at the profile's own: none, not
        // unreadable.
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("sessions");
        fs::write(&file, b"").unwrap();
        assert!(matches!(
            read_session_records(&file, "rec", &pid_text, &mut || 0),
            Read::Present(v) if v.is_empty()
        ));
        assert!(matches!(
            read_session_records(&file.join("sessions"), "rec", &pid_text, &mut || 0),
            Read::Present(v) if v.is_empty()
        ));
    }

    #[test]
    fn a_supervisor_lock_is_read_as_a_record_and_absent_or_unreadable_otherwise() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("keeper.lock");
        assert!(matches!(
            read_supervisor_lock(&lock, &pid_text),
            Read::Absent
        ));
        assert!(
            matches!(
                read_supervisor_lock(&d.path().join("no-such-dir/keeper.lock"), &pid_text),
                Read::Absent
            ),
            "nothing is written under a profile that is missing"
        );
        fs::write(&lock, b"77").unwrap();
        assert!(matches!(
            read_supervisor_lock(&lock, &pid_text),
            Read::Present(77)
        ));
        for bytes in [&b"{"[..], b"seven", b"", b"0"] {
            fs::write(&lock, bytes).unwrap();
            let Read::Unreadable(e) = read_supervisor_lock(&lock, &pid_text) else {
                panic!("{bytes:?} is unreadable");
            };
            assert!(e.what.ends_with("keeper.lock"), "{e:?}");
        }
        fs::remove_file(&lock).unwrap();
        fs::create_dir(&lock).unwrap();
        assert!(
            matches!(read_supervisor_lock(&lock, &pid_text), Read::Unreadable(_)),
            "a directory is no lock file"
        );
    }

    fn running(start_time_s: i64) -> FakeProcess {
        FakeProcess {
            exists: Some(true),
            start_time_s: Some(start_time_s),
            ..FakeProcess::default()
        }
    }

    fn rec(start: ProcStart, started_at_ms: Option<i64>) -> SessionRecord {
        SessionRecord {
            pid: 100,
            start,
            started_at_ms,
            session_id: None,
            background: false,
        }
    }

    const STARTED: i64 = 1_790_858_096;

    /// A running process that does not mention the launch command, started at `start_time_s`.
    fn stranger(start_time_s: i64) -> FakeProcess {
        FakeProcess {
            mentions_launch: Some(false),
            ..running(start_time_s)
        }
    }

    #[test]
    fn an_epoch_start_of_another_program_is_live_only_within_a_second_of_the_os_start_time() {
        for (actual, live) in [
            (STARTED, true),
            (STARTED - 1, true),
            (STARTED + 1, true),
            (STARTED - 2, false),
            (STARTED + 2, false),
            (STARTED + 3_600, false),
        ] {
            let probe = FakeProcessProbe::new();
            probe.set(100, stranger(actual));
            assert_eq!(
                record_is_live(&probe, &rec(ProcStart::EpochS(STARTED), None), "claude"),
                live,
                "OS start {actual}"
            );
        }
    }

    #[test]
    fn a_recycled_pid_left_by_sigkill_is_dead_when_it_is_not_a_claude_process() {
        // The record outlived its writer, and the OS gave the pid to a new process an hour
        // later. The start mismatches and the process is not a claude process, so the record is
        // dead (Decision 16).
        let probe = FakeProcessProbe::new();
        probe.set(100, stranger(STARTED + 3_600));
        assert!(!record_is_live(
            &probe,
            &rec(ProcStart::EpochS(STARTED), Some(STARTED * 1000)),
            "claude"
        ));
    }

    #[test]
    fn an_epoch_start_mismatch_is_recycled_only_when_the_process_does_not_mention_the_launch_command()
     {
        // Decision 16: a Linux start time moves with the wall clock (suspend, a clock step), so
        // a mismatch alone is no proof. A process that mentions `claude` is live, and so is one
        // whose arguments cannot be read (§12.6: undetermined counts as live).
        for (mentions, live) in [(Some(true), true), (Some(false), false), (None, true)] {
            let probe = FakeProcessProbe::new();
            probe.set(
                100,
                FakeProcess {
                    mentions_launch: mentions,
                    ..running(STARTED + 3_600)
                },
            );
            assert_eq!(
                record_is_live(
                    &probe,
                    &rec(ProcStart::EpochS(STARTED), Some(STARTED * 1000)),
                    "claude"
                ),
                live,
                "mentions {mentions:?}"
            );
        }
    }

    #[test]
    fn a_pid_that_is_gone_is_dead_and_one_that_cannot_be_probed_is_live() {
        let probe = FakeProcessProbe::new();
        assert!(
            !record_is_live(&probe, &rec(ProcStart::EpochS(STARTED), None), "claude"),
            "unknown pid"
        );
        probe.set(
            100,
            FakeProcess {
                exists: Some(false),
                ..running(STARTED)
            },
        );
        assert!(!record_is_live(
            &probe,
            &rec(ProcStart::EpochS(STARTED), None),
            "claude"
        ));
        probe.set(
            100,
            FakeProcess {
                exists: None,
                ..running(STARTED + 3_600)
            },
        );
        assert!(record_is_live(
            &probe,
            &rec(ProcStart::EpochS(STARTED), None),
            "claude"
        ));
        probe.set(
            100,
            FakeProcess {
                start_time_s: None,
                ..running(0)
            },
        );
        assert!(
            record_is_live(&probe, &rec(ProcStart::EpochS(STARTED), None), "claude"),
            "an unknown start time counts as live"
        );
    }

    #[test]
    fn a_boot_tick_start_must_equal_the_start_ticks() {
        for (ticks, live) in [(Some(5_000), true), (Some(5_001), false), (None, true)] {
            let probe = FakeProcessProbe::new();
            probe.set(
                100,
                FakeProcess {
                    exists: Some(true),
                    start_ticks: ticks,
                    ..FakeProcess::default()
                },
            );
            assert_eq!(
                record_is_live(&probe, &rec(ProcStart::BootTicks(5_000), None), "claude"),
                live,
                "{ticks:?}"
            );
        }
    }

    #[test]
    fn without_a_known_start_cswaps_heuristic_decides() {
        // With no known start, a pid is recycled only if its process started more than 120 s
        // after `startedAt` and nothing about it mentions the launch command.
        let at_ms = STARTED * 1000;
        let cases = [
            (STARTED + 121, Some(false), Some(at_ms), true, false),
            (STARTED + 121, Some(true), Some(at_ms), true, true),
            (STARTED + 121, None, Some(at_ms), true, true),
            (STARTED + 120, Some(false), Some(at_ms), true, true),
            (STARTED + 5, Some(false), Some(at_ms), true, true),
            (STARTED + 121, Some(false), None, true, true),
            (STARTED + 121, Some(false), Some(at_ms), false, true),
        ];
        for (start, mentions, started_at, known_start, live) in cases {
            let probe = FakeProcessProbe::new();
            probe.set(
                100,
                FakeProcess {
                    exists: Some(true),
                    start_time_s: known_start.then_some(start),
                    start_ticks: None,
                    mentions_launch: mentions,
                    ppid: None,
                },
            );
            assert_eq!(
                record_is_live(&probe, &rec(ProcStart::Unknown, started_at), "claude"),
                live,
                "start {start}, mentions {mentions:?}, startedAt {started_at:?}, \
                 known {known_start}"
            );
        }
    }

    #[test]
    fn an_indeterminate_boot_tick_start_is_live_whatever_the_ticks() {
        for ticks in [Some(5_000), Some(0), None] {
            let probe = FakeProcessProbe::new();
            probe.set(
                100,
                FakeProcess {
                    start_ticks: ticks,
                    ..stranger(STARTED + 3_600)
                },
            );
            assert!(
                record_is_live(
                    &probe,
                    &rec(ProcStart::BootTicksIndeterminate, Some(STARTED * 1000)),
                    "claude"
                ),
                "{ticks:?}"
            );
        }
        let probe = FakeProcessProbe::new();
        probe.set(
            100,
            FakeProcess {
                exists: Some(false),
                ..FakeProcess::default()
            },
        );
        assert!(!record_is_live(
            &probe,
            &rec(ProcStart::BootTicksIndeterminate, None),
            "claude"
        ));
    }

    #[test]
    fn fake_probe_reports_its_ppid() {
        let probe = FakeProcessProbe::new();
        probe.set(
            100,
            FakeProcess {
                ppid: Some(42),
                ..running(STARTED)
            },
        );
        probe.set(101, running(STARTED));
        assert_eq!(probe.parent_pid(100), Some(42));
        assert_eq!(probe.parent_pid(101), None, "unknown when not set");
        assert_eq!(probe.parent_pid(102), None, "an unknown pid has no parent");
    }

    #[test]
    fn procargs_yield_the_executable_and_arguments_but_never_the_environment() {
        let mut buf = 2_i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/usr/local/bin/node\0\0\0\0");
        buf.extend_from_slice(b"node\0/opt/claude-code/cli.js\0");
        buf.extend_from_slice(b"CLAUDE_CONFIG_DIR=/p\0");
        assert_eq!(
            parse_procargs(&buf).unwrap(),
            vec![
                b"/usr/local/bin/node".to_vec(),
                b"node".to_vec(),
                b"/opt/claude-code/cli.js".to_vec()
            ]
        );
        let mut env_only = 0_i32.to_ne_bytes().to_vec();
        env_only.extend_from_slice(b"/bin/sh\0claude=1\0");
        assert_eq!(
            parse_procargs(&env_only).unwrap(),
            vec![b"/bin/sh".to_vec()]
        );
        assert_eq!(parse_procargs(b"\x01"), None, "too short for argc");
        assert_eq!(
            parse_procargs(&1_i32.to_ne_bytes()),
            None,
            "no executable path"
        );
    }

    #[test]
    fn this_process_is_live_under_its_own_start_time_and_recycled_under_another() {
        let me = std::process::id();
        let p = SystemProcessProbe;
        assert_eq!(p.exists(me), Some(true));
        let start = p.start_time_s(me).expect("this process's start time");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert!(
            start <= now + 1 && now - start < 86_400,
            "{start} vs now {now}"
        );
        // The launch command is a word this process never mentions, so a mismatch is decided by
        // the start time alone (Decision 16), wherever the test binary was built.
        let stranger = "no-such-word-7f3a";
        let mine = SessionRecord {
            pid: me,
            start: ProcStart::EpochS(start),
            started_at_ms: Some(start * 1000),
            session_id: None,
            background: false,
        };
        assert!(record_is_live(&p, &mine, stranger));
        let recycled = SessionRecord {
            start: ProcStart::EpochS(start - 3_600),
            ..mine.clone()
        };
        assert!(!record_is_live(&p, &recycled, stranger));
        assert!(
            record_is_live(&p, &recycled, "tagteam_provider"),
            "a mismatch on a process that mentions the launch command is live"
        );
    }

    #[test]
    fn this_process_mentions_its_own_binary_and_not_a_stranger() {
        let me = std::process::id();
        let p = SystemProcessProbe;
        assert_eq!(p.mentions(me, "tagteam_provider"), Some(true));
        assert_eq!(p.mentions(me, "no-such-word-7f3a"), Some(false));
        if cfg!(target_os = "linux") {
            assert!(p.start_ticks(me).is_some());
        } else {
            assert_eq!(p.start_ticks(me), None);
        }
    }

    #[test]
    fn a_pid_no_process_can_hold_is_gone_and_unprobeable_pids_are_undetermined() {
        let p = SystemProcessProbe;
        let dead = i32::MAX as u32;
        assert_eq!(p.exists(dead), Some(false));
        assert_eq!(p.start_time_s(dead), None);
        assert_eq!(p.start_ticks(dead), None);
        assert_eq!(p.mentions(dead, "claude"), None);
        assert_eq!(p.exists(0), None, "pid 0 is a process group");
        assert_eq!(p.exists(u32::MAX), None);
        let gone = SessionRecord {
            pid: dead,
            start: ProcStart::Unknown,
            started_at_ms: None,
            session_id: None,
            background: false,
        };
        assert!(!record_is_live(&p, &gone, "claude"));
    }
}
