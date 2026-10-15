//! Shared by the socket tests (`server.rs`, `client.rs`, `umask.rs`). Each test file is its own
//! crate and uses only part of this module, so an item unused by one of them is not dead code
//! overall.
#![allow(dead_code)]

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use tagteam_core::ProviderId;
use tagteam_daemon::events::Mode;
use tagteam_daemon::status::{DaemonInfo, NotifyInfo, ProviderStatus, StatusBoard};
use tagteam_engine::settings::NotifyEvent;

/// The home `status` writes paths under (D5.4).
pub const HOME: &str = "/Users/tester";
/// 2026-09-21T14:13:20Z, the fixture daemon's start.
pub const T: i64 = 1_790_000_000;

/// A temporary directory short enough for a socket path under it (D14). `sun_path` holds 104
/// bytes on macOS, and macOS's own `$TMPDIR` can take most of them. It is made under `/tmp`,
/// else under `$TMPDIR` where `/tmp` cannot be written (a sandbox).
///
/// It is chmod'ed 0700 after it is made: a bind in another test of this binary may have
/// narrowed the umask to 0177 at that instant, which would leave the directory 0600 and
/// unusable.
pub fn short_tempdir() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("tt")
        .tempdir_in("/tmp")
        .or_else(|_| tempfile::Builder::new().prefix("tt").tempdir())
        .expect("a temporary directory under /tmp or $TMPDIR");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        dir.path().as_os_str().len() <= 48,
        "{} is too long for socket tests; set TMPDIR to a short directory",
        dir.path().display()
    );
    dir
}

/// The fixture daemon: pid 4242, started at `T`, logging under `HOME`.
pub fn daemon_info() -> DaemonInfo {
    DaemonInfo {
        pid: 4242,
        version: "20261014.2045.0".into(),
        started_at: T,
        socket: None,
        log: Path::new(HOME).join(".local/state/tagteam/tagteam.log"),
    }
}

/// A board with notifications on and one provider in `auto`.
pub fn board() -> Arc<StatusBoard> {
    let board = StatusBoard::new(
        daemon_info(),
        NotifyInfo {
            enabled: true,
            backend: Some("osascript"),
            events: vec![
                NotifyEvent::Switch,
                NotifyEvent::Quarantine,
                NotifyEvent::Limit,
            ],
        },
    );
    board.set_provider(ProviderStatus {
        provider: ProviderId::new("claude-code"),
        mode: Mode::Auto,
        autoswitch: true,
        waiting_for: None,
        holder_pid: None,
        backoff: None,
        last_tick_at: Some(T),
        next_tick_at: Some(T + 300),
        last_outcome: None,
        last_switch: None,
    });
    board
}

/// Polls `ready` every 10 ms; fails the test, naming `what`, if it does not hold within 10 s.
pub fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "never: {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// A raw client of the socket, which reads and writes lines itself.
pub struct Peer {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Peer {
    /// Connected, with nothing read yet. A read waits 10 s at most.
    pub fn connect(path: &Path) -> Peer {
        let stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        Peer {
            writer: stream.try_clone().unwrap(),
            reader: BufReader::new(stream),
        }
    }

    /// Connected, with the greeting read.
    pub fn greeted(path: &Path) -> Peer {
        let mut peer = Peer::connect(path);
        let hello = peer.line();
        assert_eq!(hello["event"], "hello", "{hello}");
        peer
    }

    /// Greeted and subscribed with the string id `id`, its `ok` read.
    pub fn subscribed(path: &Path, id: &str) -> Peer {
        let mut peer = Peer::greeted(path);
        peer.send(&format!(r#"{{"id":"{id}","op":"subscribe"}}"#));
        let ok = peer.line();
        assert_eq!(ok["ok"], true, "{ok}");
        peer
    }

    /// `line`, then its newline.
    pub fn send(&mut self, line: &str) {
        self.send_bytes(format!("{line}\n").as_bytes()).unwrap();
    }

    /// Raw bytes. It fails once the server has closed the connection.
    pub fn send_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)
    }

    /// The next line, or `None` at the end of the connection. A reset, or a last line cut short
    /// by the close, counts as the end. It fails the test when nothing comes within 10 s.
    pub fn next_line(&mut self) -> Option<Value> {
        let mut buf = String::new();
        match self.reader.read_line(&mut buf) {
            Ok(0) => None,
            Ok(_) if buf.ends_with('\n') => Some(serde_json::from_str(&buf).unwrap()),
            Ok(_) => None,
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => None,
            Err(e) => panic!("no line within 10 s: {e}"),
        }
    }

    /// The next line; it fails the test at the end of the connection.
    pub fn line(&mut self) -> Value {
        self.next_line()
            .expect("a line, not the end of the connection")
    }

    /// Every line left, up to the end of the connection.
    pub fn rest(&mut self) -> Vec<Value> {
        std::iter::from_fn(|| self.next_line()).collect()
    }
}
