//! What a Claude Code background daemon leaves in a compat home (Appendix A.7), and how the
//! harness tells that it is gone. In 2.1.292 the supervisor writes no session record: its
//! identity is `<home>/daemon.lock` (`pid`, `origin`, `procStart`) and `daemon/roster.json`
//! (`supervisorPid`, and a pid for every worker), and each worker writes a session record. The
//! roster keeps stale pids after a shutdown, so a pid counts only when it still belongs to the
//! process that was recorded (`record_is_live`, §12.6): an exited or recycled one does not.
//!
//! Nothing here signals a process. The daemon is stopped by `claude daemon stop --any` in its
//! own home (`Ctx::stop_daemon`), and this module only looks.

use std::fs;
use std::io;
use std::path::Path;

use serde_json::{Value, json};
use tagteam_provider::{
    LockProbe, ProcessProbe, Read, RecordEntry, SessionRecord, launch_reservations, parse_lstart,
    parse_session_record, read_session_records, read_supervisor_lock, record_is_live,
};

use super::ctx::DAEMON_KINDS;

/// The command whose name a recorded process's arguments mention (§12.6).
const LAUNCH: &str = "claude";

/// What is left of a home's daemon: processes that still belong to it, and files that cannot be
/// read, which may hide one.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Survey {
    pub alive: Vec<String>,
    pub unreadable: Vec<String>,
}

impl Survey {
    /// Nothing runs and nothing is unreadable.
    pub fn is_clear(&self) -> bool {
        self.alive.is_empty() && self.unreadable.is_empty()
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.alive.is_empty() {
            parts.push(format!("still running: {}", self.alive.join(", ")));
        }
        if !self.unreadable.is_empty() {
            parts.push(format!("cannot be read: {}", self.unreadable.join(", ")));
        }
        parts.join("; ")
    }
}

/// A pid the roster names, whose process is the recorded one: judged by the `procStart` or
/// `startedAt` beside it when the roster has one, and otherwise by the process still being a
/// `claude` one (a roster pid with no start time of its own could be anyone's by now).
fn roster_pid_lives(probe: &dyn ProcessProbe, record: &SessionRecord) -> bool {
    record_is_live(probe, record, LAUNCH)
        && (record.proc_start.is_some()
            || record.started_at_ms.is_some()
            || probe.mentions(record.pid, LAUNCH) != Some(false))
}

/// What `daemon/roster.json` names, judged as a worker's own record is: each entry under
/// `workers` (an object's values or an array's elements) by its `pid` and any start time beside
/// it, and its repl process (`replPid`, with `replProcStart`) the same way. Whatever does not
/// parse is returned as unreadable, since it may hide a process: an entry that is no object
/// with a pid, a `replPid` or a `supervisorPid` that is no integer (`null` is no pid). The
/// roster's `supervisorPid` is checked for its type only: a bare pid in a file that keeps stale
/// ones proves nothing, and the live supervisor is `daemon.lock`'s (`read_lock`).
fn roster_records(roster: &Value) -> (Vec<(String, SessionRecord)>, Vec<String>) {
    let (mut out, mut unreadable) = (Vec::new(), Vec::new());
    match &roster["supervisorPid"] {
        Value::Null => {}
        v if v.as_u64().and_then(|p| u32::try_from(p).ok()).is_some() => {}
        _ => unreadable.push("daemon/roster.json (supervisorPid is no pid)".to_owned()),
    }
    let entries: Vec<&Value> = match &roster["workers"] {
        Value::Object(o) => o.values().collect(),
        Value::Array(a) => a.iter().collect(),
        _ => Vec::new(),
    };
    for entry in entries {
        match parse_session_record(entry.to_string().as_bytes()) {
            Ok(r) => out.push(("a worker in the roster".to_owned(), r)),
            Err(detail) => {
                unreadable.push(format!("a worker in daemon/roster.json ({detail})"));
                continue;
            }
        }
        match &entry["replPid"] {
            Value::Null => {}
            v => match v.as_u64().and_then(|p| u32::try_from(p).ok()) {
                Some(pid) => {
                    let proc_start = match &entry["replProcStart"] {
                        Value::String(s) => Some(s.clone()),
                        Value::Number(n) => Some(n.to_string()),
                        _ => None,
                    };
                    out.push((
                        "a worker's repl in the roster".to_owned(),
                        SessionRecord {
                            pid,
                            proc_start,
                            started_at_ms: None,
                            kind: None,
                        },
                    ));
                }
                None => {
                    unreadable.push("a worker in daemon/roster.json (replPid is no pid)".to_owned())
                }
            },
        }
    }
    (out, unreadable)
}

/// `daemon.lock` (CC 2.1.292) into `s`: a live supervisor is alive, a lock that cannot be read
/// is unreadable, as tagteam judges both (§12.6).
fn read_lock(home: &Path, probe: &dyn ProcessProbe, s: &mut Survey) {
    match read_supervisor_lock(&home.join("daemon.lock")) {
        Read::Present(r) if record_is_live(probe, &r, LAUNCH) => {
            s.alive
                .push(format!("the supervisor in daemon.lock (pid {})", r.pid));
        }
        Read::Present(_) | Read::Absent => {}
        Read::Unreadable(e) => s.unreadable.push(format!("daemon.lock ({e})")),
    }
}

/// The home's session records into `s`: a live one is alive (`only_daemons` keeps to the
/// daemon kinds, `DAEMON_KINDS`), one that cannot be read is unreadable.
fn read_records(home: &Path, probe: &dyn ProcessProbe, only_daemons: bool, s: &mut Survey) {
    match read_session_records(&home.join("sessions")) {
        Read::Present(entries) => {
            for entry in entries {
                match entry {
                    RecordEntry::Record(r) => {
                        let daemon = r.kind.as_deref().is_some_and(|k| DAEMON_KINDS.contains(&k));
                        if (daemon || !only_daemons) && record_is_live(probe, &r, LAUNCH) {
                            s.alive.push(format!(
                                "a record of kind {} (pid {})",
                                r.kind.as_deref().unwrap_or("session"),
                                r.pid
                            ));
                        }
                    }
                    RecordEntry::Unreadable { path, detail } => {
                        s.unreadable.push(format!("{} ({detail})", path.display()))
                    }
                }
            }
        }
        Read::Absent => {}
        Read::Unreadable(e) => s.unreadable.push(format!("sessions ({e})")),
    }
}

/// Everything of the home's daemon that still lives: `daemon.lock`'s pid, the roster's
/// supervisor and worker pids, and every session record of a daemon kind (`DAEMON_KINDS`).
pub fn survey(home: &Path, probe: &dyn ProcessProbe) -> Survey {
    let mut s = Survey::default();
    read_lock(home, probe, &mut s);

    match fs::read(home.join("daemon/roster.json")) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(roster) => {
                let (records, unreadable) = roster_records(&roster);
                s.unreadable.extend(unreadable);
                for (what, r) in records {
                    if roster_pid_lives(probe, &r) {
                        s.alive.push(format!("{what} (pid {})", r.pid));
                    }
                }
            }
            Err(_) => s
                .unreadable
                .push("daemon/roster.json (it is not JSON)".to_owned()),
        },
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) => {}
        Err(e) => s.unreadable.push(format!("daemon/roster.json ({e})")),
    }

    read_records(home, probe, true, &mut s);
    s.alive.sort();
    s.alive.dedup();
    s
}

/// What makes the profile `home` session-owned for tagteam (§12.5, §12.6): a launch
/// reservation that is held, a live session record of any kind, a live daemon supervisor in
/// `daemon.lock`; and a reservation, record or lock that cannot be read, which may hide one.
pub fn owners(home: &Path, probe: &dyn ProcessProbe) -> Survey {
    let mut s = Survey::default();
    match launch_reservations(home) {
        Read::Present(held) => {
            for (path, state) in held {
                if state == LockProbe::Held {
                    s.alive.push(format!(
                        "a launch reservation ({})",
                        path.file_name()
                            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                    ));
                }
            }
        }
        Read::Absent => {}
        Read::Unreadable(e) => s.unreadable.push(format!("launch reservations ({e})")),
    }
    read_records(home, probe, false, &mut s);
    read_lock(home, probe, &mut s);
    s.alive.sort();
    s.alive.dedup();
    s
}

/// Whether `ran` is an installed `claude` rejecting `--any` as an option it does not know, so
/// that plain `daemon stop` is what that version has.
pub fn rejects_any(stderr: &str, stdout: &str) -> bool {
    let text = format!("{stderr}\n{stdout}").to_lowercase();
    text.contains("unknown option") && text.contains("--any")
}

/// What kind of text a `procStart` is, never its value: the next live run reads this off
/// `daemon.lock` to confirm that it is judged as a session record's is.
fn proc_start_class(v: Option<&Value>) -> &'static str {
    match v {
        None => "absent",
        Some(Value::String(s)) if parse_lstart(s.trim()).is_some() => "lstart text",
        Some(Value::String(s)) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
            "digit string"
        }
        Some(Value::String(_)) => "other text",
        Some(Value::Number(_)) => "number",
        Some(Value::Null) => "null",
        Some(_) => "other type",
    }
}

/// `daemon.lock`'s shape as evidence: whether it reads, its key names, and the format class of
/// its `procStart`. Never any other value.
pub fn lock_shape(home: &Path) -> Value {
    let raw = match fs::read(home.join("daemon.lock")) {
        Ok(raw) => raw,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return json!({"read": "absent"});
        }
        Err(_) => return json!({"read": "unreadable"}),
    };
    match serde_json::from_slice::<Value>(&raw) {
        Ok(Value::Object(o)) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            json!({"read": "present", "keys": keys, "procStart": proc_start_class(o.get("procStart"))})
        }
        _ => json!({"read": "unreadable"}),
    }
}

#[cfg(test)]
mod tests {
    use tagteam_provider::{FakeProcess, FakeProcessProbe};

    use super::*;

    const LSTART: &str = "Thu Oct  1 12:34:56 2026";
    /// `LSTART` in epoch seconds, as `record_is_live` reads it.
    const STARTED: i64 = 1_790_858_096;

    fn home(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xtask-daemon-{tag}-{}-{}",
            std::process::id(),
            crate::compat::keychain::random_hex().unwrap()
        ));
        fs::create_dir_all(dir.join("daemon")).unwrap();
        fs::create_dir_all(dir.join("sessions")).unwrap();
        dir
    }

    fn running(probe: &FakeProcessProbe, pid: u32, started: i64) {
        probe.set(
            pid,
            FakeProcess {
                exists: Some(true),
                start_time_s: Some(started),
                mentions_launch: Some(true),
                ..FakeProcess::default()
            },
        );
    }

    fn lock(home: &Path, pid: u32) {
        fs::write(
            home.join("daemon.lock"),
            json!({"pid": pid, "origin": "transient", "procStart": LSTART}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn a_supervisor_in_daemon_lock_counts_only_while_its_process_is_the_recorded_one() {
        let h = home("lock");
        let probe = FakeProcessProbe::new();
        lock(&h, 100);

        // Live: the pid runs, and started when the lock says.
        running(&probe, 100, STARTED);
        let s = survey(&h, &probe);
        assert_eq!(s.alive, ["the supervisor in daemon.lock (pid 100)"]);
        assert!(!s.is_clear());

        // Dead: nothing runs under the pid.
        let dead = FakeProcessProbe::new();
        assert!(survey(&h, &dead).is_clear());

        // Reused: the pid runs, but another program started it long after.
        let reused = FakeProcessProbe::new();
        reused.set(
            100,
            FakeProcess {
                exists: Some(true),
                start_time_s: Some(STARTED + 7_200),
                mentions_launch: Some(false),
                ..FakeProcess::default()
            },
        );
        assert!(survey(&h, &reused).is_clear(), "{:?}", survey(&h, &reused));

        // A pid that cannot be probed counts as running: it fails closed.
        let unknown = FakeProcessProbe::new();
        unknown.set(100, FakeProcess::default());
        assert!(!survey(&h, &unknown).is_clear());
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn an_unreadable_lock_roster_or_record_is_not_clear_and_a_missing_one_is() {
        let probe = FakeProcessProbe::new();
        let h = home("unreadable");
        assert!(survey(&h, &probe).is_clear(), "no lock, roster or record");
        fs::write(h.join("daemon.lock"), "not json").unwrap();
        let s = survey(&h, &probe);
        assert!(
            s.alive.is_empty() && s.unreadable.len() == 1 && !s.is_clear(),
            "{s:?}"
        );
        assert!(s.describe().contains("cannot be read: daemon.lock"));
        fs::remove_file(h.join("daemon.lock")).unwrap();

        fs::write(h.join("daemon/roster.json"), "{").unwrap();
        let s = survey(&h, &probe);
        assert_eq!(s.unreadable, ["daemon/roster.json (it is not JSON)"]);
        fs::write(h.join("daemon/roster.json"), "{}").unwrap();
        assert!(survey(&h, &probe).is_clear(), "valid JSON naming no pid");
        fs::remove_file(h.join("daemon/roster.json")).unwrap();

        fs::write(h.join("sessions/1.json"), "[]").unwrap();
        assert!(!survey(&h, &probe).is_clear());
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn the_roster_s_stale_pids_are_dead_and_its_live_ones_are_not() {
        let h = home("roster");
        fs::write(
            h.join("daemon/roster.json"),
            json!({
                "supervisorPid": 200,
                "workers": {
                    "a": {"pid": 201, "procStart": LSTART},
                    "b": {"pid": 202, "procStart": LSTART},
                    "c": {"pid": 203},
                    "d": {"pid": 204},
                }
            })
            .to_string(),
        )
        .unwrap();
        let probe = FakeProcessProbe::new();
        // 200: gone. 201: live. 202: reused (started hours later, not claude). 203: no start
        // time and not claude, so someone else's. 204: no start time and claude.
        running(&probe, 201, STARTED);
        probe.set(
            202,
            FakeProcess {
                exists: Some(true),
                start_time_s: Some(STARTED + 9_000),
                mentions_launch: Some(false),
                ..FakeProcess::default()
            },
        );
        probe.set(
            203,
            FakeProcess {
                exists: Some(true),
                mentions_launch: Some(false),
                ..FakeProcess::default()
            },
        );
        probe.set(
            204,
            FakeProcess {
                exists: Some(true),
                mentions_launch: Some(true),
                ..FakeProcess::default()
            },
        );
        let s = survey(&h, &probe);
        assert_eq!(
            s.alive,
            [
                "a worker in the roster (pid 201)",
                "a worker in the roster (pid 204)"
            ]
        );

        // A bare supervisorPid proves nothing: with no daemon.lock it is not counted, though
        // the pid runs (the roster keeps stale pids).
        fs::write(
            h.join("daemon/roster.json"),
            json!({"supervisorPid": 205, "workers": [{"pid": 206}]}).to_string(),
        )
        .unwrap();
        let probe = FakeProcessProbe::new();
        running(&probe, 205, STARTED);
        assert!(survey(&h, &probe).is_clear(), "{:?}", survey(&h, &probe));

        // With a daemon.lock naming it, the supervisor is the lock's, judged by its procStart,
        // and counted once.
        lock(&h, 205);
        assert_eq!(
            survey(&h, &probe).alive,
            ["the supervisor in daemon.lock (pid 205)"]
        );
        fs::remove_file(h.join("daemon.lock")).unwrap();
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn a_worker_s_repl_process_counts_by_its_own_start_time() {
        let h = home("repl");
        fs::write(
            h.join("daemon/roster.json"),
            json!({"workers": {
                "a": {"pid": 211, "procStart": LSTART, "replPid": 212, "replProcStart": LSTART},
                "b": {"pid": 213, "procStart": LSTART, "replPid": 214, "replProcStart": LSTART},
            }})
            .to_string(),
        )
        .unwrap();
        let probe = FakeProcessProbe::new();
        // The workers are gone; repl 212 is the recorded process, and 214 was recycled.
        running(&probe, 212, STARTED);
        probe.set(
            214,
            FakeProcess {
                exists: Some(true),
                start_time_s: Some(STARTED + 9_000),
                mentions_launch: Some(false),
                ..FakeProcess::default()
            },
        );
        let s = survey(&h, &probe);
        assert_eq!(s.alive, ["a worker's repl in the roster (pid 212)"]);
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn a_roster_entry_that_does_not_parse_cannot_be_verified() {
        let h = home("roster-bad");
        let probe = FakeProcessProbe::new();
        for (roster, named) in [
            (
                json!({"workers": {"a": {"no": "pid"}}}),
                "a worker in daemon/roster.json",
            ),
            (
                json!({"workers": ["nope"]}),
                "a worker in daemon/roster.json",
            ),
            (
                json!({"workers": {"a": {"pid": 221, "replPid": "x"}}}),
                "replPid is no pid",
            ),
            (json!({"supervisorPid": "x"}), "supervisorPid is no pid"),
            (json!({"supervisorPid": -4}), "supervisorPid is no pid"),
        ] {
            fs::write(h.join("daemon/roster.json"), roster.to_string()).unwrap();
            let s = survey(&h, &probe);
            assert!(
                !s.is_clear() && s.describe().contains(named),
                "{roster}: {s:?}"
            );
        }
        // null is no pid: a roster without a supervisor yet.
        fs::write(
            h.join("daemon/roster.json"),
            json!({"supervisorPid": null, "workers": {}}).to_string(),
        )
        .unwrap();
        assert!(survey(&h, &probe).is_clear());
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn only_a_live_record_of_a_daemon_kind_counts() {
        let h = home("records");
        for (name, record) in [
            ("1", json!({"pid": 301, "kind": "bg", "procStart": LSTART})),
            (
                "2",
                json!({"pid": 302, "kind": "daemon-worker", "procStart": LSTART}),
            ),
            (
                "3",
                json!({"pid": 303, "kind": "interactive", "procStart": LSTART}),
            ),
            ("4", json!({"pid": 304, "kind": "bg", "procStart": LSTART})),
        ] {
            fs::write(h.join(format!("sessions/{name}.json")), record.to_string()).unwrap();
        }
        let probe = FakeProcessProbe::new();
        for pid in [301, 302, 303] {
            running(&probe, pid, STARTED);
        }
        let s = survey(&h, &probe);
        assert_eq!(
            s.alive,
            [
                "a record of kind bg (pid 301)",
                "a record of kind daemon-worker (pid 302)"
            ]
        );
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn a_profile_is_owned_by_a_held_reservation_any_live_record_or_a_live_supervisor() {
        use tagteam_provider::FlockGuard;
        let h = home("owners");
        let probe = FakeProcessProbe::new();
        assert!(owners(&h, &probe).is_clear());

        // A live record of any kind owns the profile, though it is no daemon's.
        fs::write(
            h.join("sessions/7.json"),
            json!({"pid": 401, "kind": "interactive", "procStart": LSTART}).to_string(),
        )
        .unwrap();
        running(&probe, 401, STARTED);
        assert_eq!(
            owners(&h, &probe).alive,
            ["a record of kind interactive (pid 401)"]
        );
        assert!(survey(&h, &probe).is_clear(), "but it is no daemon's");
        fs::remove_file(h.join("sessions/7.json")).unwrap();
        assert!(owners(&h, &FakeProcessProbe::new()).is_clear());

        // A held reservation does; a free one does not.
        fs::create_dir_all(h.join(".tagteam-launch")).unwrap();
        let path = h.join(".tagteam-launch/run.lock");
        fs::write(&path, "").unwrap();
        assert!(owners(&h, &probe).is_clear(), "a free reservation");
        let held = FlockGuard::try_lock(&path)
            .unwrap()
            .expect("the lock is free");
        assert_eq!(
            owners(&h, &probe).alive,
            ["a launch reservation (run.lock)"]
        );
        drop(held);

        // A live supervisor does, and an unreadable lock may hide one.
        lock(&h, 100);
        running(&probe, 100, STARTED);
        assert_eq!(
            owners(&h, &probe).alive,
            ["the supervisor in daemon.lock (pid 100)"]
        );
        fs::write(h.join("daemon.lock"), "{").unwrap();
        assert_eq!(owners(&h, &probe).unreadable.len(), 1);
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn an_older_claude_rejecting_any_is_told_from_any_other_failure() {
        assert!(rejects_any("error: unknown option '--any'\n", ""));
        assert!(rejects_any("", "Error: Unknown option --any"));
        assert!(!rejects_any("no daemon is running\n", ""));
        assert!(!rejects_any("error: unknown option '--keep-workers'", ""));
    }

    #[test]
    fn the_lock_s_shape_is_its_key_names_and_its_proc_start_s_class_and_no_value() {
        let h = home("shape");
        assert_eq!(lock_shape(&h), json!({"read": "absent"}));
        fs::write(
            h.join("daemon.lock"),
            json!({"pid": 424242, "origin": "transient", "procStart": LSTART}).to_string(),
        )
        .unwrap();
        let shape = lock_shape(&h);
        assert_eq!(
            shape,
            json!({"read": "present", "keys": ["origin", "pid", "procStart"],
                   "procStart": "lstart text"})
        );
        let shown = shape.to_string();
        assert!(
            !shown.contains("424242") && !shown.contains("transient") && !shown.contains("2026"),
            "{shown}"
        );
        for (value, class) in [
            (json!("123456789"), "digit string"),
            (json!(123_456), "number"),
            (json!("whenever"), "other text"),
            (json!(null), "null"),
            (json!([1]), "other type"),
        ] {
            fs::write(
                h.join("daemon.lock"),
                json!({"procStart": value}).to_string(),
            )
            .unwrap();
            assert_eq!(lock_shape(&h)["procStart"], class);
        }
        fs::write(h.join("daemon.lock"), r#"{"pid":1}"#).unwrap();
        assert_eq!(lock_shape(&h)["procStart"], "absent");
        fs::write(h.join("daemon.lock"), "nope").unwrap();
        assert_eq!(lock_shape(&h), json!({"read": "unreadable"}));
        fs::remove_dir_all(&h).unwrap();
    }
}
