//! §12.6: session records and whether the process that wrote one still lives. Reservations are
//! judged by their lock alone (§12.5, `flock::probe_lock`); this is the record side.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde_json::Value;

use crate::read::{Read, ReadError};

/// One `<config_home>/sessions/<pid>.json` (Appendix A.7), reduced to what liveness needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub pid: u32,
    /// `ps -o lstart=` text, or the all-digit legacy form; `None` when absent or not text.
    pub proc_start: Option<String>,
    /// `startedAt`, epoch ms.
    pub started_at_ms: Option<i64>,
    pub kind: Option<String>,
}

/// A record nested deeper than this is malformed (§12.6 "deep nesting").
const MAX_DEPTH: usize = 32;

fn depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// `pid` as a process id `kill` may be given: 1 through `i32::MAX`. Zero and negative values
/// would address a process group, so they are out of range, never a pid to probe.
fn pid_of(v: Option<&Value>) -> Result<u32, String> {
    let Some(v) = v else {
        return Err("it has no pid".into());
    };
    let Value::Number(n) = v else {
        return Err("its pid is not an integer".into());
    };
    if let Some(p) = n.as_u64() {
        return u32::try_from(p)
            .ok()
            .filter(|p| (1..=i32::MAX as u32).contains(p))
            .ok_or_else(|| "its pid is out of range".to_owned());
    }
    let text = n.to_string();
    let integral = text
        .strip_prefix('-')
        .unwrap_or(&text)
        .bytes()
        .all(|b| b.is_ascii_digit());
    if integral {
        Err("its pid is out of range".into())
    } else {
        Err("its pid is not an integer".into())
    }
}

/// §12.6: non-object JSON, a missing or non-integer or out-of-range `pid`, depth > 32, or
/// invalid UTF-8 is `Err(detail)`; unknown fields are ignored. `detail` never quotes the bytes.
pub fn parse_session_record(bytes: &[u8]) -> Result<SessionRecord, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "it is not UTF-8".to_owned())?;
    let v: Value = serde_json::from_str(text).map_err(|_| "it is not JSON".to_owned())?;
    let Value::Object(o) = &v else {
        return Err("it is not a JSON object".into());
    };
    if depth(&v) > MAX_DEPTH {
        return Err(format!("it is nested deeper than {MAX_DEPTH} levels"));
    }
    let pid = pid_of(o.get("pid"))?;
    let proc_start = match o.get("procStart") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    Ok(SessionRecord {
        pid,
        proc_start,
        started_at_ms: o.get("startedAt").and_then(Value::as_i64),
        kind: o.get("kind").and_then(Value::as_str).map(str::to_owned),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordEntry {
    Record(SessionRecord),
    /// Present but not a readable record (§12.6: it counts as unreadable). `detail` never
    /// quotes the file's bytes.
    Unreadable {
        path: PathBuf,
        detail: String,
    },
}

/// Every `*.json` in `dir`, in name order. A missing `dir` is `Present(vec![])`, and so is one
/// that is not a directory, or under a profile path that is not one (`ENOTDIR`): no record can
/// be written there. A `dir` that cannot be listed is `Unreadable`. A record that vanishes between the listing and its read
/// has been removed by its session's exit, so it is skipped.
pub fn read_session_records(dir: &Path) -> Read<Vec<RecordEntry>> {
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
                if path.extension().is_some_and(|x| x == "json") {
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
        match fs::read(&path) {
            Ok(bytes) => out.push(match parse_session_record(&bytes) {
                Ok(r) => RecordEntry::Record(r),
                Err(detail) => RecordEntry::Unreadable { path, detail },
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => out.push(RecordEntry::Unreadable {
                path,
                detail: e.to_string(),
            }),
        }
    }
    Read::Present(out)
}

/// A background supervisor's lock file (CC 2.1.292: `<profile>/daemon.lock`, JSON with at least
/// `pid`, `origin` and `procStart`), read as a session record is (§12.6): the supervisor's `pid`
/// and `procStart` are what liveness needs. A missing file, or one under a profile path that is
/// not a directory, is `Absent`; one that cannot be read or parsed is `Unreadable`, and counts
/// as owned like an unreadable record. `detail` never quotes the file's bytes.
pub fn read_supervisor_lock(path: &Path) -> Read<SessionRecord> {
    let unreadable =
        |detail: String| Read::Unreadable(ReadError::new(path.display().to_string(), detail));
    match fs::read(path) {
        Ok(bytes) => match parse_session_record(&bytes) {
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

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `s` as a number of `min..=max` ASCII digits; no sign, no spaces.
fn digits(s: &str, min: usize, max: usize) -> Option<u32> {
    let ok = (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| s.parse().ok()).flatten()
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to the proleptic Gregorian date `y-m-d` (H. Hinnant's
/// `days_from_civil`). `m` is 1–12 and `d` is valid for the month.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `ps -o lstart=` with `LC_ALL=C TZ=UTC` (`Thu Oct  1 12:34:56 2026`) to epoch seconds. Runs of
/// spaces separate the fields, as `ps` pads a single-digit day. The weekday name must be one of
/// the seven but is not checked against the date. `None` for anything else.
pub fn parse_lstart(s: &str) -> Option<i64> {
    let fields: Vec<&str> = s.split_whitespace().collect();
    let [weekday, month, day, time, year] = fields.as_slice() else {
        return None;
    };
    if !WEEKDAYS.contains(weekday) {
        return None;
    }
    let month = MONTHS.iter().position(|m| m == month)? as u32 + 1;
    let year = i64::from(digits(year, 4, 4)?);
    if year < 1970 {
        return None;
    }
    let day = digits(day, 1, 2)?;
    if day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let hms: Vec<&str> = time.split(':').collect();
    let [h, m, sec] = hms.as_slice() else {
        return None;
    };
    let (h, m, sec) = (digits(h, 2, 2)?, digits(m, 2, 2)?, digits(sec, 2, 2)?);
    if h > 23 || m > 59 || sec > 59 {
        return None;
    }
    Some(
        days_from_civil(year, month, day) * 86_400
            + i64::from(h) * 3_600
            + i64::from(m) * 60
            + i64::from(sec),
    )
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
#[derive(Debug, Clone, Default)]
pub struct FakeProcess {
    pub exists: Option<bool>,
    pub start_time_s: Option<i64>,
    pub start_ticks: Option<u64>,
    /// `mentions`' answer, whatever the needle.
    pub mentions_launch: Option<bool>,
}

/// Tests only (no feature gate: it touches nothing). Unknown pids are `exists: Some(false)`.
#[derive(Default)]
pub struct FakeProcessProbe {
    procs: Mutex<BTreeMap<u32, FakeProcess>>,
}

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
}

/// How long after `startedAt` a process must have started, by cswap's rule, before it can be
/// judged a recycled pid (§12.6).
const RECYCLE_GRACE_MS: i64 = 120_000;

/// §12.6: live if the pid exists and still belongs to the record's writer. `launch_command`
/// is the provider's (`claude`), used by the heuristic for an absent `procStart` and, under
/// Decision 16, to judge an `lstart` mismatch (recycled only if the process does not mention it).
/// Anything that cannot be determined counts as live.
pub fn record_is_live(probe: &dyn ProcessProbe, r: &SessionRecord, launch_command: &str) -> bool {
    match probe.exists(r.pid) {
        Some(false) => return false,
        None => return true,
        Some(true) => {}
    }
    let proc_start = r.proc_start.as_deref().map(str::trim).unwrap_or("");
    if let Some(recorded) = parse_lstart(proc_start) {
        // The OS start time equal to the record's within ±1 s, or unknown, is live. A mismatch
        // alone is no proof (Decision 16): a Linux start time moves with the wall clock, so it
        // is recycled only when the process does not mention the launch command either. An
        // unknown answer counts as mentioning it.
        let matches = probe
            .start_time_s(r.pid)
            .is_none_or(|actual| (actual - recorded).abs() <= 1);
        return matches || probe.mentions(r.pid, launch_command).unwrap_or(true);
    }
    if !proc_start.is_empty() && proc_start.bytes().all(|b| b.is_ascii_digit()) {
        return match (proc_start.parse::<u64>().ok(), probe.start_ticks(r.pid)) {
            (Some(recorded), Some(actual)) => actual == recorded,
            _ => true,
        };
    }
    // Absent or unparseable: recycled only if the process started more than 120 s after
    // `startedAt` and neither its executable name nor its arguments mention the launch command.
    let started_late = match (r.started_at_ms, probe.start_time_s(r.pid)) {
        // Whole seconds round the start down, so this never calls a process late early.
        (Some(at_ms), Some(start_s)) => {
            start_s.saturating_mul(1000) > at_ms.saturating_add(RECYCLE_GRACE_MS)
        }
        _ => false,
    };
    !started_late || probe.mentions(r.pid, launch_command) != Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    fn record(v: Value) -> Result<SessionRecord, String> {
        parse_session_record(v.to_string().as_bytes())
    }

    #[test]
    fn a_record_keeps_what_liveness_needs_and_ignores_the_rest() {
        let r = record(json!({
            "pid": 4242, "sessionId": "s", "cwd": "/w", "startedAt": 1_790_858_096_000_i64,
            "procStart": "Thu Oct  1 12:34:56 2026", "version": "2.1.286", "kind": "bg",
            "entrypoint": "cli", "status": "idle", "updatedAt": 1
        }))
        .unwrap();
        assert_eq!(
            r,
            SessionRecord {
                pid: 4242,
                proc_start: Some("Thu Oct  1 12:34:56 2026".into()),
                started_at_ms: Some(1_790_858_096_000),
                kind: Some("bg".into()),
            }
        );
        assert_eq!(
            record(json!({"pid": 7})).unwrap(),
            SessionRecord {
                pid: 7,
                proc_start: None,
                started_at_ms: None,
                kind: None
            }
        );
    }

    #[test]
    fn a_numeric_proc_start_is_the_all_digit_legacy_form_and_any_other_shape_is_absent() {
        assert_eq!(
            record(json!({"pid": 7, "procStart": 123456}))
                .unwrap()
                .proc_start
                .as_deref(),
            Some("123456")
        );
        for odd in [json!(null), json!(true), json!({"t": 1}), json!([1])] {
            assert_eq!(
                record(json!({"pid": 7, "procStart": odd}))
                    .unwrap()
                    .proc_start,
                None
            );
        }
        assert_eq!(
            record(json!({"pid": 7, "startedAt": "soon", "kind": 3})).unwrap(),
            SessionRecord {
                pid: 7,
                proc_start: None,
                started_at_ms: None,
                kind: None
            }
        );
    }

    #[test]
    fn a_malformed_record_is_an_error_that_never_quotes_its_bytes() {
        let nested = |levels: usize| {
            let mut s = String::from("{\"pid\": 7, \"x\": ");
            s.push_str(&"[".repeat(levels));
            s.push_str(&"]".repeat(levels));
            s.push('}');
            s.into_bytes()
        };
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("not JSON", b"SENTINEL{".to_vec()),
            ("an array", br#"["SENTINEL"]"#.to_vec()),
            ("a string", br#""SENTINEL""#.to_vec()),
            ("a number", b"7".to_vec()),
            ("no pid", br#"{"kind": "SENTINEL"}"#.to_vec()),
            ("a string pid", br#"{"pid": "SENTINEL"}"#.to_vec()),
            ("a fractional pid", br#"{"pid": 1.5}"#.to_vec()),
            ("pid 0", br#"{"pid": 0}"#.to_vec()),
            ("a negative pid", br#"{"pid": -1}"#.to_vec()),
            ("a pid above i32::MAX", br#"{"pid": 2147483648}"#.to_vec()),
            (
                "a huge pid",
                br#"{"pid": 99999999999999999999999999}"#.to_vec(),
            ),
            ("33 levels", nested(32)),
            ("200 levels", nested(199)),
            (
                "invalid UTF-8",
                b"{\"pid\": 7, \"k\": \"\xff\xfeSENTINEL\"}".to_vec(),
            ),
        ];
        for (case, bytes) in cases {
            let detail = parse_session_record(&bytes).expect_err(case);
            assert!(!detail.contains("SENTINEL"), "{case}: {detail}");
        }
        assert_eq!(
            parse_session_record(&nested(31)).unwrap().pid,
            7,
            "32 levels is the limit, not past it"
        );
        assert_eq!(
            record(json!({"pid": i32::MAX})).unwrap().pid,
            i32::MAX as u32
        );
    }

    #[test]
    fn records_are_listed_in_name_order_and_a_malformed_one_is_unreadable() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("sessions");
        assert!(
            matches!(read_session_records(&dir), Read::Present(v) if v.is_empty()),
            "a missing directory holds no records"
        );
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("20.json"), br#"{"pid": 20}"#).unwrap();
        fs::write(dir.join("10.json"), br#"{"pid": 10}"#).unwrap();
        fs::write(dir.join("30.json"), b"{").unwrap();
        fs::write(dir.join("notes.txt"), b"not a record").unwrap();
        fs::create_dir(dir.join("40.json")).unwrap();
        let Read::Present(entries) = read_session_records(&dir) else {
            panic!("the directory lists");
        };
        let pid = |p| RecordEntry::Record(record(json!({"pid": p})).unwrap());
        assert_eq!(entries[..2], [pid(10), pid(20)]);
        assert!(
            matches!(&entries[2], RecordEntry::Unreadable { path, .. } if path == &dir.join("30.json"))
        );
        assert!(
            matches!(&entries[3], RecordEntry::Unreadable { path, .. } if path == &dir.join("40.json")),
            "a directory named like a record cannot be read"
        );
        assert_eq!(entries.len(), 4, "notes.txt is not a record");
    }

    #[test]
    fn a_records_directory_that_cannot_be_listed_is_unreadable() {
        // Run as a non-root user: root lists a 0o000 directory.
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("sessions");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
        let found = read_session_records(&dir);
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
        assert!(matches!(read_session_records(&file), Read::Present(v) if v.is_empty()));
        assert!(matches!(
            read_session_records(&file.join("sessions")),
            Read::Present(v) if v.is_empty()
        ));
    }

    #[test]
    fn a_supervisor_lock_is_read_as_a_record_and_absent_or_unreadable_otherwise() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("daemon.lock");
        assert!(matches!(read_supervisor_lock(&lock), Read::Absent));
        assert!(
            matches!(
                read_supervisor_lock(&d.path().join("no-such-dir/daemon.lock")),
                Read::Absent
            ),
            "nothing is written under a profile that is missing"
        );
        fs::write(
            &lock,
            json!({"pid": 77, "origin": "transient", "procStart": "Thu Oct  1 12:34:56 2026"})
                .to_string(),
        )
        .unwrap();
        let Read::Present(r) = read_supervisor_lock(&lock) else {
            panic!("a supervisor lock reads");
        };
        assert_eq!(r.pid, 77);
        assert_eq!(r.proc_start.as_deref(), Some("Thu Oct  1 12:34:56 2026"));
        for bytes in [
            &b"{"[..],
            b"[1]",
            br#"{"origin": "transient"}"#,
            br#"{"pid": 0}"#,
        ] {
            fs::write(&lock, bytes).unwrap();
            let Read::Unreadable(e) = read_supervisor_lock(&lock) else {
                panic!("{bytes:?} is unreadable");
            };
            assert!(e.what.ends_with("daemon.lock"), "{e:?}");
        }
        fs::remove_file(&lock).unwrap();
        fs::create_dir(&lock).unwrap();
        assert!(
            matches!(read_supervisor_lock(&lock), Read::Unreadable(_)),
            "a directory is no lock file"
        );
    }

    #[test]
    fn lstart_text_parses_to_epoch_seconds() {
        for (text, epoch) in [
            ("Thu Jan  1 00:00:00 1970", 0),
            ("Thu Oct  1 12:34:56 2026", 1_790_858_096),
            ("Thu Oct 10 01:02:03 2030", 1_917_824_523),
            ("Fri Dec 31 23:59:59 1999", 946_684_799),
            ("Thu Feb 29 23:59:59 2024", 1_709_251_199),
            ("Tue Feb 29 00:00:00 2000", 951_782_400),
            ("Mon Mar  1 00:00:00 2100", 4_107_542_400),
            ("Thu Oct  1 12:34:56 2026\n", 1_790_858_096),
            ("  Thu Oct  1 12:34:56 2026  ", 1_790_858_096),
            // The weekday is not checked against the date: 2026-10-01 is a Thursday.
            ("Wed Oct  1 12:34:56 2026", 1_790_858_096),
        ] {
            assert_eq!(parse_lstart(text), Some(epoch), "{text:?}");
        }
    }

    #[test]
    fn anything_but_lstart_text_is_refused() {
        for text in [
            "",
            "garbage",
            "1790858096",
            "Mon Feb 29 00:00:00 2100",
            "Wed Feb 29 00:00:00 2023",
            "Thu Apr 31 00:00:00 2026",
            "Thu Oct  0 12:34:56 2026",
            "Thu Oct 32 12:34:56 2026",
            "Thu Oct +1 12:34:56 2026",
            "Thu Oct 001 12:34:56 2026",
            "Thu Foo  1 12:34:56 2026",
            "Xyz Oct  1 12:34:56 2026",
            "thu oct  1 12:34:56 2026",
            "Thu Oct  1 24:00:00 2026",
            "Thu Oct  1 12:60:00 2026",
            "Thu Oct  1 12:34:60 2026",
            "Thu Oct  1 12:34 2026",
            "Thu Oct  1 1:02:03 2026",
            "Thu Oct  1 12:34:56 1969",
            "Thu Oct  1 12:34:56 -2026",
            "Thu Oct  1 12:34:56 26",
            "Thu Oct  1 12:34:56 2026 UTC",
        ] {
            assert_eq!(parse_lstart(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_civil_arithmetic_agrees_with_every_day_from_1970_to_2400() {
        let mut expected = 0;
        for y in 1970..2400 {
            for m in 1..=12 {
                for d in 1..=days_in_month(y, m) {
                    assert_eq!(days_from_civil(y, m, d), expected, "{y}-{m}-{d}");
                    expected += 1;
                }
            }
        }
    }

    fn running(start_time_s: i64) -> FakeProcess {
        FakeProcess {
            exists: Some(true),
            start_time_s: Some(start_time_s),
            ..FakeProcess::default()
        }
    }

    fn rec(proc_start: Option<&str>, started_at_ms: Option<i64>) -> SessionRecord {
        SessionRecord {
            pid: 100,
            proc_start: proc_start.map(str::to_owned),
            started_at_ms,
            kind: Some("interactive".into()),
        }
    }

    const LSTART: &str = "Thu Oct  1 12:34:56 2026";
    const STARTED: i64 = 1_790_858_096;

    /// A running process that does not mention the launch command, started at `start_time_s`.
    fn stranger(start_time_s: i64) -> FakeProcess {
        FakeProcess {
            mentions_launch: Some(false),
            ..running(start_time_s)
        }
    }

    #[test]
    fn an_lstart_record_of_another_program_is_live_only_within_a_second_of_the_os_start_time() {
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
                record_is_live(&probe, &rec(Some(LSTART), None), "claude"),
                live,
                "OS start {actual}"
            );
        }
    }

    #[test]
    fn a_recycled_pid_left_by_sigkill_is_dead_when_it_is_not_a_claude_process() {
        // Review Focus 2: the record outlived its writer, and the OS gave the pid to a new
        // process an hour later. The lstart mismatches and the process is not a claude
        // process, so the record is dead (Decision 16).
        let probe = FakeProcessProbe::new();
        probe.set(100, stranger(STARTED + 3_600));
        assert!(!record_is_live(
            &probe,
            &rec(Some(LSTART), Some(STARTED * 1000)),
            "claude"
        ));
    }

    #[test]
    fn an_lstart_mismatch_is_recycled_only_when_the_process_does_not_mention_the_launch_command() {
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
                record_is_live(&probe, &rec(Some(LSTART), Some(STARTED * 1000)), "claude"),
                live,
                "mentions {mentions:?}"
            );
        }
    }

    #[test]
    fn a_pid_that_is_gone_is_dead_and_one_that_cannot_be_probed_is_live() {
        let probe = FakeProcessProbe::new();
        assert!(
            !record_is_live(&probe, &rec(Some(LSTART), None), "claude"),
            "unknown pid"
        );
        probe.set(
            100,
            FakeProcess {
                exists: Some(false),
                ..running(STARTED)
            },
        );
        assert!(!record_is_live(&probe, &rec(Some(LSTART), None), "claude"));
        probe.set(
            100,
            FakeProcess {
                exists: None,
                ..running(STARTED + 3_600)
            },
        );
        assert!(record_is_live(&probe, &rec(Some(LSTART), None), "claude"));
        probe.set(
            100,
            FakeProcess {
                start_time_s: None,
                ..running(0)
            },
        );
        assert!(
            record_is_live(&probe, &rec(Some(LSTART), None), "claude"),
            "an unknown start time counts as live"
        );
    }

    #[test]
    fn an_all_digit_proc_start_must_equal_the_start_ticks() {
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
                record_is_live(&probe, &rec(Some("5000"), None), "claude"),
                live,
                "{ticks:?}"
            );
        }
    }

    #[test]
    fn without_a_usable_proc_start_cswaps_heuristic_decides() {
        // Review Focus 2: recycled only if the process started more than 120 s after
        // `startedAt` and nothing about it mentions the launch command.
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
            for proc_start in [None, Some(""), Some("not a date")] {
                let probe = FakeProcessProbe::new();
                probe.set(
                    100,
                    FakeProcess {
                        exists: Some(true),
                        start_time_s: known_start.then_some(start),
                        start_ticks: None,
                        mentions_launch: mentions,
                    },
                );
                assert_eq!(
                    record_is_live(&probe, &rec(proc_start, started_at), "claude"),
                    live,
                    "start {start}, mentions {mentions:?}, startedAt {started_at:?}, \
                     known {known_start}, procStart {proc_start:?}"
                );
            }
        }
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

    /// The `lstart` text for an epoch second, for the real-process test below.
    fn lstart_of(epoch_s: i64) -> String {
        let (days, secs) = (epoch_s.div_euclid(86_400), epoch_s.rem_euclid(86_400));
        let (mut y, mut d) = (1970, days);
        while d >= if is_leap(y) { 366 } else { 365 } {
            d -= if is_leap(y) { 366 } else { 365 };
            y += 1;
        }
        let mut m = 1;
        while d >= i64::from(days_in_month(y, m)) {
            d -= i64::from(days_in_month(y, m));
            m += 1;
        }
        format!(
            "{} {} {:>2} {:02}:{:02}:{:02} {y}",
            WEEKDAYS[(days + 4).rem_euclid(7) as usize],
            MONTHS[m as usize - 1],
            d + 1,
            secs / 3_600,
            secs / 60 % 60,
            secs % 60
        )
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
        assert_eq!(parse_lstart(&lstart_of(start)), Some(start));
        // The launch command is a word this process never mentions, so a mismatch is decided by
        // the start time alone (Decision 16), wherever the test binary was built.
        let stranger = "no-such-word-7f3a";
        let mine = SessionRecord {
            pid: me,
            proc_start: Some(lstart_of(start)),
            started_at_ms: Some(start * 1000),
            kind: None,
        };
        assert!(record_is_live(&p, &mine, stranger));
        let recycled = SessionRecord {
            proc_start: Some(lstart_of(start - 3_600)),
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
            proc_start: None,
            started_at_ms: None,
            kind: None,
        };
        assert!(!record_is_live(&p, &gone, "claude"));
    }
}
