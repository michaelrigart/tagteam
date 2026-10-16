//! D5.7: the client, against the real server (Task 2) and against fake daemons for what the
//! real one never does. Run outside a sandbox that refuses `bind` on Unix sockets.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_core::autoswitch::Trigger;
use tagteam_daemon::client::{Client, ClientError, Connect, connect, connect_at};
use tagteam_daemon::events::{DaemonEvent, DaemonSink, Mode, StopReason, WaitingFor};
use tagteam_daemon::protocol::{Event, Hello, MAX_LINE, PROTOCOL, Status, status_json};
use tagteam_daemon::server::{MAX_CONNECTIONS, MAX_SOCKET_PATH, Server};
use tagteam_engine::auto::AutoEvent;
use tagteam_engine::daemon::daemon_socket_path;
use tagteam_provider::{Cancel, Env};

use common::{HOME, Peer, board, short_tempdir, wait_until};

/// The fixture daemon's greeting (`common::daemon_info`).
const HELLO: &str = r#"{"schemaVersion":1,"event":"hello","protocol":1,"version":"20261014.2045.0","pid":4242,"startedAt":"2026-09-21T14:13:20Z"}"#;

fn serve(path: &Path) -> Server {
    Server::bind(path, Path::new(HOME), board(), Cancel::new()).unwrap()
}

/// The `Client` a connect gave, failing the test on any other answer.
fn connected(answer: Result<Connect, ClientError>) -> Client {
    match answer {
        Ok(Connect::Connected(client)) => client,
        other => panic!("not connected: {other:?}"),
    }
}

fn assert_no_daemon(answer: Result<Connect, ClientError>, case: &str) {
    match answer {
        Ok(Connect::NoDaemon) => {}
        other => panic!("{case}: {other:?}"),
    }
}

/// A fake daemon's side of its one connection.
struct Fake {
    reader: BufReader<UnixStream>,
    stream: UnixStream,
}

impl Fake {
    fn write(&mut self, line: &str) {
        self.stream
            .write_all(format!("{line}\n").as_bytes())
            .unwrap();
    }

    /// The next request, as JSON.
    fn request(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
}

/// A fake daemon at `path` that serves one connection with `script`, on a thread of its own.
fn fake(path: &Path, script: impl FnOnce(Fake) + Send + 'static) -> JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        script(Fake {
            reader: BufReader::new(stream.try_clone().unwrap()),
            stream,
        });
    })
}

#[test]
fn connect_reads_the_greeting_and_status_answers_from_the_board() {
    // D5.7: `Connected`, with the greeting typed; `status` (D5.4) typed, as often as asked.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let server = serve(&path);
    let mut client = connected(connect_at(&path));
    assert_eq!(
        client.hello(),
        &Hello {
            protocol: PROTOCOL,
            version: "20261014.2045.0".into(),
            pid: 4242,
            started_at: "2026-09-21T14:13:20Z".into(),
        }
    );
    let status = client.status().unwrap();
    assert_eq!(status.daemon.pid, 4242);
    assert_eq!(status.daemon.protocol, PROTOCOL);
    assert_eq!(status.daemon.log, "~/.local/state/tagteam/tagteam.log");
    assert_eq!(status.notifications.backend.as_deref(), Some("osascript"));
    assert_eq!(status.providers.len(), 1);
    assert_eq!(status.providers[0].provider, "claude-code");
    assert_eq!(status.providers[0].mode, "auto");
    assert_eq!(status.subscribers, 0);
    assert_eq!(
        client.status().unwrap(),
        status,
        "a second request, with the next id, is answered too"
    );
    drop(client);
    server.shutdown();
}

#[test]
fn connect_finds_the_socket_from_the_environment_alone() {
    // D5.1, D1.1 criterion 5: `$XDG_DATA_HOME/tagteam/daemon.sock`, from the Env, with no
    // settings read.
    let dir = short_tempdir();
    let env = Env::for_test(dir.path());
    let server =
        Server::bind(&daemon_socket_path(&env), &env.home, board(), Cancel::new()).unwrap();
    assert_eq!(connected(connect(&env)).hello().pid, 4242);
    server.shutdown();
    assert_no_daemon(connect(&env), "after shutdown");
}

#[test]
fn a_missing_socket_is_no_daemon() {
    let dir = short_tempdir();
    assert_no_daemon(connect_at(&dir.path().join("daemon.sock")), "no file");
    assert_no_daemon(
        connect_at(&dir.path().join("absent/daemon.sock")),
        "no directory",
    );
}

#[test]
fn a_socket_path_too_long_to_bind_is_no_daemon() {
    // D5.7, D5.1: no daemon can listen at a path over `MAX_SOCKET_PATH`, so `connect` answers
    // NoDaemon, not the `InvalidInput` the operating system gives.
    let dir = short_tempdir();
    let room = MAX_SOCKET_PATH - dir.path().as_os_str().len();
    let long = dir.path().join("d".repeat(room)).join("daemon.sock");
    assert!(long.as_os_str().len() > MAX_SOCKET_PATH);
    assert_no_daemon(connect_at(&long), "connect_at");
    let mut env = Env::for_test(dir.path());
    env.xdg_data_home = Some(dir.path().join("d".repeat(room)));
    assert!(daemon_socket_path(&env).as_os_str().len() > MAX_SOCKET_PATH);
    assert_no_daemon(connect(&env), "connect");
}

#[test]
fn a_socket_without_a_listener_is_no_daemon() {
    // Review Focus 1: the file can exist with nobody listening, before start-up binds or once
    // the accept thread has gone. `connect` answers NoDaemon at once; it never hangs or
    // fails.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let quick = |case: &str| {
        let started = Instant::now();
        assert_no_daemon(connect_at(&path), case);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{case}: {:?}",
            started.elapsed()
        );
    };
    drop(UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    quick("a socket file whose listener is gone");
    fs::remove_file(&path).unwrap();
    fs::write(&path, "").unwrap();
    quick("a file that is not a socket");
    fs::remove_file(&path).unwrap();
    // A daemon that is stopping: its accept thread went on the signal, and the file stays
    // until `stopping` or shutdown removes it.
    let cancel = Cancel::new();
    let server = Server::bind(&path, Path::new(HOME), board(), cancel.clone()).unwrap();
    cancel.request(libc::SIGTERM);
    wait_until(
        "the accept thread gone",
        || matches!(UnixStream::connect(&path), Err(e) if e.kind() == io::ErrorKind::ConnectionRefused),
    );
    assert!(path.exists());
    quick("a stopping daemon");
    server.shutdown();
    quick("a daemon that has stopped");
}

#[test]
fn the_end_of_the_connection_before_the_greeting_is_no_daemon() {
    // D5.7: a listener that closes before it greets, as a daemon at its limit does (D5.1).
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let closing = fake(&path, drop);
    assert_no_daemon(connect_at(&path), "closed before the greeting");
    closing.join().unwrap();

    let path = dir.path().join("daemon.sock");
    let server = serve(&path);
    let full: Vec<Peer> = (0..MAX_CONNECTIONS).map(|_| Peer::greeted(&path)).collect();
    assert_no_daemon(connect_at(&path), "the seventeenth connection");
    drop(full);
    server.shutdown();
}

#[test]
fn another_protocol_is_a_mismatch() {
    // D5.6: a client that does not speak the greeting's protocol closes the connection and
    // works without the daemon. Nothing else of that greeting is read.
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let daemon = fake(&path, |mut f| {
        f.write(r#"{"schemaVersion":1,"event":"hello","protocol":2,"version":"20270101.0000.0","renamed":4242}"#);
        let mut rest = String::new();
        assert_eq!(
            f.reader.read_line(&mut rest).unwrap(),
            0,
            "the client closes"
        );
    });
    match connect_at(&path) {
        Ok(Connect::Mismatch { server, client }) => assert_eq!((server, client), (2, PROTOCOL)),
        other => panic!("{other:?}"),
    }
    daemon.join().unwrap();
}

#[test]
fn a_first_line_that_is_not_a_greeting_is_a_protocol_error() {
    let dir = short_tempdir();
    let firsts = [
        "not json",
        r#"{"schemaVersion":1,"event":"changed","ts":"2026-09-21T14:13:20Z"}"#,
        r#"{"schemaVersion":1,"event":"hello","version":"x","pid":1,"startedAt":"x"}"#,
        r#"{"schemaVersion":1,"event":"hello","protocol":1,"version":"x","startedAt":"x"}"#,
    ];
    for (n, first) in firsts.into_iter().enumerate() {
        let path = dir.path().join(format!("fake{n}.sock"));
        let daemon = fake(&path, move |mut f| f.write(first));
        match connect_at(&path) {
            Err(ClientError::Protocol(_)) => {}
            other => panic!("{first}: {other:?}"),
        }
        daemon.join().unwrap();
    }
}

#[test]
fn a_daemon_that_does_not_greet_within_250_ms_times_out() {
    // D5.7: the greeting read is bounded. A daemon that accepts and says nothing is there but
    // hung: `Timeout`, not `NoDaemon`, so `daemon status` can say so (D8.2).
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let daemon = fake(&path, |f| {
        thread::sleep(Duration::from_secs(1));
        drop(f);
    });
    let started = Instant::now();
    let answer = connect_at(&path);
    let waited = started.elapsed();
    assert!(matches!(answer, Err(ClientError::Timeout)), "{answer:?}");
    assert!(
        waited >= Duration::from_millis(200) && waited < Duration::from_millis(900),
        "{waited:?}"
    );
    daemon.join().unwrap();
}

#[test]
fn status_passes_over_events_and_other_replies_to_its_own() {
    // D5.3: replies carry `id` and events `event`; the client takes the reply with its id.
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let status = status_json(&board().snapshot(), Path::new(HOME));
    let result = status.clone();
    let daemon = fake(&path, move |mut f| {
        f.write(HELLO);
        let request = f.request();
        assert_eq!(request["op"], "status");
        assert!(request["id"].is_i64(), "{request}");
        f.write(r#"{"schemaVersion":1,"event":"changed","ts":"2026-09-21T14:13:20Z"}"#);
        f.write(r#"{"schemaVersion":1,"id":"someone else's","ok":true,"result":{}}"#);
        f.write(
            &json!({"schemaVersion": 1, "id": request["id"], "ok": true, "result": result})
                .to_string(),
        );
    });
    let mut client = connected(connect_at(&path));
    assert_eq!(
        client.status().unwrap(),
        serde_json::from_value::<Status>(status).unwrap()
    );
    daemon.join().unwrap();
}

#[test]
fn a_refused_request_is_a_refused_error() {
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let daemon = fake(&path, |mut f| {
        f.write(HELLO);
        let request = f.request();
        f.write(
            &json!({"schemaVersion": 1, "id": request["id"], "ok": false,
                    "error": {"type": "bad-request", "message": "no"}})
            .to_string(),
        );
    });
    let mut client = connected(connect_at(&path));
    match client.status() {
        Err(ClientError::Refused { kind, message }) => {
            assert_eq!((kind.as_str(), message.as_str()), ("bad-request", "no"));
        }
        other => panic!("{other:?}"),
    }
    daemon.join().unwrap();
}

#[test]
fn status_waits_two_seconds_at_most() {
    // D5.7: `status` waits up to 2 s for its reply.
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let daemon = fake(&path, |mut f| {
        f.write(HELLO);
        f.request();
        thread::sleep(Duration::from_secs(3));
    });
    let mut client = connected(connect_at(&path));
    let started = Instant::now();
    let answer = client.status();
    let waited = started.elapsed();
    assert!(matches!(answer, Err(ClientError::Timeout)), "{answer:?}");
    assert!(
        waited >= Duration::from_millis(1900) && waited < Duration::from_millis(2900),
        "{waited:?}"
    );
    drop(client);
    daemon.join().unwrap();
}

#[test]
fn subscribe_yields_typed_events_in_order_until_the_daemon_stops() {
    // D5.5, D5.7: every event, typed, in order; the iterator ends when the daemon closes the
    // connection after `stopping` (D4.6).
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let server = serve(&path);
    let events = connected(connect_at(&path)).subscribe().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for e in events {
            if tx.send(e.map_err(|e| e.to_string())).is_err() {
                return;
            }
        }
    });
    let cc = || ProviderId::new("claude-code");
    let sent = [
        DaemonEvent::Changed,
        DaemonEvent::Mode {
            provider: cc(),
            mode: Mode::Waiting,
            waiting_for: Some(WaitingFor::EngineLock),
            holder_pid: Some(5151),
            backoff: None,
        },
        DaemonEvent::Usage {
            provider: cc(),
            updated: vec![2],
            fetch_errors: BTreeMap::new(),
        },
        DaemonEvent::Auto(AutoEvent::Switch {
            provider: cc(),
            trigger: Trigger::AtLimit,
            from: 1,
            to: 2,
            warnings: vec![],
            dry_run: false,
        }),
        DaemonEvent::Stopping {
            reason: StopReason::Signal,
        },
    ];
    for e in &sent {
        server.hub().emit(e);
    }
    server.shutdown();
    let next = || {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("an event within 5 s")
            .expect("an event the client can read")
    };
    assert_eq!(next(), Event::Changed);
    assert_eq!(
        next(),
        Event::Mode {
            provider: "claude-code".into(),
            mode: "waiting".into(),
            waiting_for: Some("engine-lock".into()),
            holder_pid: Some(5151),
            backoff: None,
        }
    );
    assert_eq!(
        next(),
        Event::Usage {
            provider: "claude-code".into(),
            updated: vec![2],
            fetch_errors: BTreeMap::new(),
        }
    );
    match next() {
        Event::Auto {
            kind,
            provider,
            raw,
        } => {
            assert_eq!(
                (kind.as_str(), provider.as_str()),
                ("switch", "claude-code")
            );
            assert_eq!(raw["trigger"], "at-limit");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        next(),
        Event::Stopping {
            reason: "signal".into()
        }
    );
    assert!(
        matches!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ),
        "the iterator ends with the connection"
    );
    reader.join().unwrap();
}

/// `line` padded with spaces to `len` bytes, its newline left out.
fn padded(line: &str, len: usize) -> Vec<u8> {
    let mut bytes = line.as_bytes().to_vec();
    bytes.resize(len, b' ');
    bytes
}

#[test]
fn a_line_over_64_kib_is_a_protocol_error_wherever_its_newline_arrives() {
    // D5.2 frames both directions: at most `MAX_LINE` bytes, the newline left out. A line of
    // exactly that is read. One byte more is refused, whether its newline comes in the same
    // write as the rest (so in the read that passes the limit) or in a later one.
    const CHANGED: &str = r#"{"schemaVersion":1,"event":"changed","ts":"2026-09-21T14:13:20Z"}"#;
    for (case, later) in [
        ("newline with the line", false),
        ("newline written later", true),
    ] {
        let dir = short_tempdir();
        let path = dir.path().join("fake.sock");
        let daemon = fake(&path, move |mut f| {
            f.write(HELLO);
            let request = f.request();
            f.write(
                &json!({"schemaVersion": 1, "id": request["id"], "ok": true, "result": {}})
                    .to_string(),
            );
            let mut fits = padded(CHANGED, MAX_LINE);
            fits.push(b'\n');
            f.stream.write_all(&fits).unwrap();
            // The client may close once it has seen too much; a failed write is no matter.
            let over = padded(CHANGED, MAX_LINE + 1);
            if later {
                let _ = f.stream.write_all(&over);
                thread::sleep(Duration::from_millis(200));
                let _ = f.stream.write_all(b"\n");
            } else {
                let _ = f
                    .stream
                    .write_all(&[over.as_slice(), b"\n".as_slice()].concat());
            }
        });
        let mut events = connected(connect_at(&path)).subscribe().unwrap();
        assert_eq!(
            events.next().unwrap().unwrap(),
            Event::Changed,
            "{case}: a line of exactly MAX_LINE bytes is read"
        );
        match events.next() {
            Some(Err(ClientError::Protocol(m))) => assert!(m.contains("64 KiB"), "{case}: {m}"),
            other => panic!("{case}: {other:?}"),
        }
        assert!(events.next().is_none(), "{case}: the stream ends");
        drop(events);
        daemon.join().unwrap();
    }
}

#[test]
fn a_kind_the_client_does_not_know_is_yielded_as_unknown() {
    // D5.6, D5.7: an added kind reaches the caller as
    // `Unknown`, whole, and the stream goes on.
    let dir = short_tempdir();
    let path = dir.path().join("fake.sock");
    let token_index =
        json!({"schemaVersion":1,"event":"some-future-kind","ts":"2026-09-21T14:13:20Z","files":3});
    let line = token_index.to_string();
    let daemon = fake(&path, move |mut f| {
        f.write(HELLO);
        let request = f.request();
        assert_eq!(request["op"], "subscribe");
        f.write(
            &json!({"schemaVersion": 1, "id": request["id"], "ok": true, "result": {}}).to_string(),
        );
        f.write(&line);
        f.write(r#"{"schemaVersion":1,"event":"changed","ts":"2026-09-21T14:13:20Z"}"#);
    });
    let events: Vec<Event> = connected(connect_at(&path))
        .subscribe()
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        events,
        [
            Event::Unknown {
                kind: "some-future-kind".into(),
                raw: token_index,
            },
            Event::Changed,
        ]
    );
    daemon.join().unwrap();
}

#[test]
fn a_closer_ends_a_stream_that_another_thread_is_reading() {
    // A subscriber that finds the daemon no longer answers (the TUI's live thread, whose read
    // waits for as long as the daemon is silent) ends its stream from another thread.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let server = serve(&path);
    let events = connected(connect_at(&path)).subscribe().unwrap();
    let closer = events.closer().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(events.count());
    });
    thread::sleep(Duration::from_millis(100));
    assert!(
        rx.try_recv().is_err(),
        "the reader waits on a silent daemon"
    );
    closer.close();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)),
        Ok(0),
        "the iterator ended, with no event"
    );
    server.shutdown();
}
