//! D4.1 and D5.1: where the daemon keeps its lock and its socket, and the record its lock
//! holds. They live in the engine (daemon plan A, Decision 2) because `doctor` reads them
//! (D10), and the engine cannot depend on the daemon crate.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use tagteam_provider::{Env, FlockGuard, ProcessStamp, Read};

use crate::auto::read_lock_record;
use crate::error::EngineError;

/// The first executable regular file called `name` on a `PATH` value (§12.1's lookup):
/// D6.2's `notify-send` and D9.1's `tagteam`. The provider crate's, re-exported.
pub use tagteam_provider::process::find_on_path;

/// D4.1: `<data>/locks/daemon.lock` (§5 gains it). A try-only `flock`, like the engine locks
/// (§11.1): one daemon per data directory.
pub fn daemon_lock_path(env: &Env) -> PathBuf {
    env.data_dir().join("locks").join("daemon.lock")
}

/// D5.1: `<data>/daemon.sock`, next to the store, derived from the environment alone.
pub fn daemon_socket_path(env: &Env) -> PathBuf {
    env.data_dir().join("daemon.sock")
}

/// D5.1: the longest socket path, in bytes, that `sockaddr_un.sun_path` holds with its NUL:
/// 108 bytes on Linux. The daemon's server refuses a longer path rather than bind a truncated
/// one, and `doctor` reports it (D10). A constant, since the engine forbids the `unsafe` that
/// sizing a `sockaddr_un` would take.
#[cfg(target_os = "linux")]
pub const MAX_SOCKET_PATH: usize = 107;

/// D5.1: the longest socket path, in bytes, that `sockaddr_un.sun_path` holds with its NUL:
/// 104 bytes on macOS and the BSDs.
#[cfg(not(target_os = "linux"))]
pub const MAX_SOCKET_PATH: usize = 103;

/// What a running daemon writes into `daemon.lock` (D4.1): its pid and start time, the start
/// taken as for the `switch_journal` holder (§12.6), and its tagteam version. Never used for
/// exclusion; `doctor` and `daemon status` read it (D8.2, D10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonRecord {
    pub pid: u32,
    pub start: u64,
    pub version: String,
}

/// Replaces the held lock file's whole content with `{"pid":N,"start":S,"version":"…"}\n`,
/// through a second descriptor: the lock stays on the first, as for an engine's record
/// (§11.1).
pub fn write_daemon_record(lock: &FlockGuard, version: &str) -> Result<(), EngineError> {
    let stamp = ProcessStamp::current()?;
    let line = format!(
        "{{\"pid\":{},\"start\":{},\"version\":{}}}\n",
        stamp.pid,
        stamp.start,
        serde_json::Value::from(version)
    );
    let mut file = OpenOptions::new().write(true).open(lock.path())?;
    file.set_len(0)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// The record `write_daemon_record` left at `path`, read without trying the lock. `Absent`:
/// no file, or an empty one. `Unreadable`: anything that is not that shape. Never proof that
/// the daemon runs: judge that by an exact pid and start-time match (§12.6).
pub fn read_daemon_record(path: &Path) -> Read<DaemonRecord> {
    read_lock_record(
        path,
        "it is not a pid, a start time and a version",
        |stamp, v| {
            Some(DaemonRecord {
                pid: stamp.pid,
                start: stamp.start,
                version: v["version"].as_str()?.to_owned(),
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn the_daemon_s_lock_and_socket_live_in_the_data_directory() {
        // D4.1, D5.1: derived from the environment alone.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        assert_eq!(
            daemon_lock_path(&env),
            env.data_dir().join("locks/daemon.lock")
        );
        assert_eq!(daemon_socket_path(&env), env.data_dir().join("daemon.sock"));
        assert!(!env.data_dir().exists(), "naming them creates nothing");
    }

    #[test]
    fn the_longest_socket_path_is_the_longest_sun_path_holds_with_its_nul() {
        // D5.1: a path of `MAX_SOCKET_PATH` bytes still fits `sockaddr_un`, and one byte more
        // does not. std's `SocketAddr` checks the bound `bind(2)` would, with no socket made.
        use std::os::unix::net::SocketAddr;
        let path = |len: usize| PathBuf::from(format!("/{}", "s".repeat(len - 1)));
        assert!(SocketAddr::from_pathname(path(MAX_SOCKET_PATH)).is_ok());
        assert!(SocketAddr::from_pathname(path(MAX_SOCKET_PATH + 1)).is_err());
    }

    #[test]
    fn the_record_is_one_json_line_that_reads_back() {
        // D4.1: `{"pid":N,"start":S,"version":"…"}`, replacing whatever a dead daemon left.
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let path = daemon_lock_path(&env);
        let lock = FlockGuard::try_lock(&path).unwrap().unwrap();
        fs::write(
            &path,
            "{\"pid\":4294967295,\"start\":18446744073709551615,\"version\":\"a dead daemon's, much longer\"}\n",
        )
        .unwrap();
        write_daemon_record(&lock, "20261014.2045.0").unwrap();
        let me = ProcessStamp::current().unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!(
                "{{\"pid\":{},\"start\":{},\"version\":\"20261014.2045.0\"}}\n",
                me.pid, me.start
            )
        );
        let expected = DaemonRecord {
            pid: me.pid,
            start: me.start,
            version: "20261014.2045.0".into(),
        };
        assert!(matches!(read_daemon_record(&path), Read::Present(r) if r == expected));
        let odd = "1.0 \"dev\" \\ build";
        write_daemon_record(&lock, odd).unwrap();
        assert!(
            matches!(read_daemon_record(&path), Read::Present(r) if r.version == odd),
            "a version is JSON-escaped"
        );
    }

    #[test]
    fn a_missing_or_empty_record_is_absent_and_any_other_shape_unreadable() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("daemon.lock");
        assert!(matches!(read_daemon_record(&path), Read::Absent));
        fs::write(&path, "").unwrap();
        assert!(matches!(read_daemon_record(&path), Read::Absent));
        for text in [
            "garbage",
            "{\"pid\":1,\"start\":2}\n",
            "{\"pid\":4294967296,\"start\":2,\"version\":\"v\"}\n",
            "{\"pid\":1,\"start\":2,\"version\":3}\n",
        ] {
            fs::write(&path, text).unwrap();
            assert!(
                matches!(read_daemon_record(&path), Read::Unreadable(_)),
                "{text}"
            );
        }
    }

    #[test]
    fn an_unreadable_daemon_record_names_its_shape() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("daemon.lock");
        fs::write(&path, "{\"pid\":1,\"start\":2}\n").unwrap();
        assert!(matches!(read_daemon_record(&path), Read::Unreadable(e)
                if e.detail == "it is not a pid, a start time and a version"));
    }

    #[test]
    fn a_daemon_record_that_cannot_be_read_keeps_its_i_o_error() {
        // A directory where the record belongs: the read fails, which is no shape to name.
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("daemon.lock");
        fs::create_dir(&path).unwrap();
        match read_daemon_record(&path) {
            Read::Unreadable(e) => {
                assert_eq!(e.what, path.display().to_string());
                assert!(!e.detail.is_empty());
                assert_ne!(e.detail, "it is not a pid, a start time and a version");
            }
            _ => panic!("a directory is an unreadable record"),
        }
    }

    #[test]
    fn find_on_path_finds_the_first_executable_of_a_name() {
        // D6.2's `notify-send`, D9.1's `tagteam`: a file that is not executable is passed over.
        let d = tempfile::tempdir().unwrap();
        let (plain, exec) = (d.path().join("plain"), d.path().join("exec"));
        for dir in [&plain, &exec] {
            fs::create_dir(dir).unwrap();
        }
        fs::write(plain.join("notify-send"), "").unwrap();
        fs::write(exec.join("notify-send"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(exec.join("notify-send"), fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([&plain, &exec]).unwrap();
        assert_eq!(
            find_on_path("notify-send", Some(path.as_os_str())),
            Some(exec.join("notify-send"))
        );
        assert_eq!(
            find_on_path("notify-send", None),
            None,
            "no PATH, nothing found"
        );
    }
}
