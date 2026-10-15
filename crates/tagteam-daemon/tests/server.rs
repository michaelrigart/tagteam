//! D5's socket server, over real sockets in short temporary directories (D14). Run outside a
//! sandbox that refuses `bind` on Unix sockets.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_daemon::events::{DaemonEvent, DaemonSink, Mode, StopReason, WaitingFor};
use tagteam_daemon::protocol::{
    MAX_LINE, RequestId, daemon_event_json, hello_json, ok_json, status_json,
};
use tagteam_daemon::server::{
    BindError, Hub, IDLE, Limits, MAX_CONNECTIONS, MAX_SOCKET_PATH, QUEUE, Server, WRITE_TIMEOUT,
    peer_uid,
};
use tagteam_daemon::status::StatusBoard;
use tagteam_engine::auto::AutoEvent;
use tagteam_provider::Cancel;

use common::{HOME, Peer, board, short_tempdir, wait_until};

/// A server on `daemon.sock` in `dir`, with the default limits, and its board.
fn start(dir: &Path) -> (Server, Arc<StatusBoard>) {
    start_with(dir, Limits::default())
}

fn start_with(dir: &Path, limits: Limits) -> (Server, Arc<StatusBoard>) {
    let board = board();
    let server = Server::bind_with(
        &dir.join("daemon.sock"),
        Path::new(HOME),
        Arc::clone(&board),
        Cancel::new(),
        limits,
    )
    .unwrap();
    (server, board)
}

/// An event line without its `ts`, which the hub stamps from the wall clock.
fn without_ts(mut v: Value) -> Value {
    v.as_object_mut().unwrap().remove("ts");
    v
}

fn mode_event() -> DaemonEvent {
    DaemonEvent::Mode {
        provider: ProviderId::new("claude-code"),
        mode: Mode::Waiting,
        waiting_for: Some(WaitingFor::EngineLock),
        holder_pid: Some(5151),
        backoff: None,
    }
}

fn usage_event() -> DaemonEvent {
    DaemonEvent::Usage {
        provider: ProviderId::new("claude-code"),
        updated: vec![1, 2],
        fetch_errors: BTreeMap::new(),
    }
}

/// Event `n` of a burst: about 2 KiB, numbered at the start of its message.
fn burst_event(n: usize) -> DaemonEvent {
    DaemonEvent::Auto(AutoEvent::Error {
        provider: ProviderId::new("claude-code"),
        message: format!("{n:06} {}", "x".repeat(2000)),
        transient: true,
    })
}

#[test]
fn the_server_speaks_first_with_the_greeting() {
    // D5.2: the greeting comes before any request.
    let dir = short_tempdir();
    let (server, board) = start(dir.path());
    let mut peer = Peer::connect(&dir.path().join("daemon.sock"));
    assert_eq!(peer.line(), hello_json(&board.snapshot().daemon));
    server.shutdown();
}

#[test]
fn status_answers_from_the_board_and_the_connection_stays_open() {
    // D5.3, D5.4: a reply carries its request's id, and its result is the board as it is
    // now, written by `status_json`.
    let dir = short_tempdir();
    let (server, board) = start(dir.path());
    let mut peer = Peer::greeted(&dir.path().join("daemon.sock"));
    peer.send(r#"{"id":1,"op":"status"}"#);
    assert_eq!(
        peer.line(),
        ok_json(
            &RequestId::Int(1),
            status_json(&board.snapshot(), Path::new(HOME))
        )
    );
    // What the scheduler writes next shows in the next reply.
    let mut moved = board.snapshot().providers[0].clone();
    moved.mode = Mode::Collect;
    board.set_provider(moved);
    peer.send(r#"{"id":"two","op":"status"}"#);
    let reply = peer.line();
    assert_eq!(reply["id"], "two");
    assert_eq!(reply["result"]["providers"][0]["mode"], "collect");
    server.shutdown();
}

#[test]
fn a_malformed_request_is_refused_and_the_connection_stays_open() {
    // Decision 1, D5.3: a request that is an object but not a usable one gets `ok: false`.
    let dir = short_tempdir();
    let (server, _board) = start(dir.path());
    let mut peer = Peer::greeted(&dir.path().join("daemon.sock"));
    peer.send(r#"{"op":"status"}"#);
    let refused = peer.line();
    assert_eq!(refused["id"], Value::Null, "{refused}");
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"]["type"], "bad-request");
    peer.send(r#"{"id":"q","op":"pause"}"#);
    let unknown = peer.line();
    assert_eq!(unknown["id"], "q", "{unknown}");
    assert_eq!(unknown["error"]["type"], "unknown-op");
    peer.send(r#"{"id":3,"op":"status"}"#);
    assert_eq!(peer.line()["ok"], true);
    server.shutdown();
}

#[test]
fn subscribe_delivers_every_event_in_order_after_its_ok() {
    // D5.5: after the ok, every event the daemon emits, as `daemon_event_json` writes it. The
    // hub keeps the board's count of subscribers.
    let dir = short_tempdir();
    let (server, board) = start(dir.path());
    let mut peer = Peer::greeted(&dir.path().join("daemon.sock"));
    assert_eq!(board.snapshot().subscribers, 0);
    peer.send(r#"{"id":"s","op":"subscribe"}"#);
    assert_eq!(
        peer.line(),
        json!({"schemaVersion": 1, "id": "s", "ok": true, "result": {}})
    );
    assert_eq!(board.snapshot().subscribers, 1);
    let sent = [
        DaemonEvent::Changed,
        mode_event(),
        usage_event(),
        burst_event(0),
        DaemonEvent::Stopping {
            reason: StopReason::Upgrade,
        },
    ];
    for e in &sent {
        server.hub().emit(e);
    }
    for e in &sent {
        assert_eq!(without_ts(peer.line()), without_ts(daemon_event_json(e, 0)));
    }
    drop(peer);
    wait_until("the subscriber gone from the board", || {
        board.snapshot().subscribers == 0
    });
    server.shutdown();
}

#[test]
fn status_and_subscribe_interleave_on_one_connection() {
    // D5.3: requests may come at any time, subscribed or not. Replies carry `id` and events
    // `event`, each written whole, in the order produced.
    let dir = short_tempdir();
    let (server, board) = start(dir.path());
    let mut peer = Peer::subscribed(&dir.path().join("daemon.sock"), "sub");
    server.hub().emit(&DaemonEvent::Changed);
    peer.send(r#"{"id":5,"op":"status"}"#);
    assert_eq!(
        peer.line()["event"],
        "changed",
        "queued before the request, so written before its reply"
    );
    let reply = peer.line();
    assert_eq!(reply["id"], 5);
    assert_eq!(reply["result"]["subscribers"], 1);
    server.hub().emit(&mode_event());
    peer.send(r#"{"id":"again","op":"subscribe"}"#);
    assert_eq!(peer.line()["event"], "mode");
    assert_eq!(
        peer.line(),
        json!({"schemaVersion": 1, "id": "again", "ok": true, "result": {}}),
        "a second subscribe is answered and changes nothing"
    );
    server.hub().emit(&DaemonEvent::Changed);
    assert_eq!(peer.line()["event"], "changed");
    peer.send(r#"{"id":6,"op":"status"}"#);
    assert_eq!(peer.line()["id"], 6, "one queue: the event came once");
    assert_eq!(board.snapshot().subscribers, 1);
    server.shutdown();
}

#[test]
fn a_stalled_subscriber_is_dropped_alone_and_the_rest_see_every_event() {
    // Review Focus 2, D5.5: sixteen subscribers, one of which never reads, while the scheduler
    // emits a burst. The stalled one's socket buffers fill, its thread blocks writing, and its
    // queue fills: the hub drops it alone and never waits for it. The fifteen others get every
    // event, in order.
    const BATCH: usize = 32;
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    // A write may block for a minute: the full queue, not the write timeout, drops it.
    let (server, board) = start_with(
        dir.path(),
        Limits {
            idle: IDLE,
            write_timeout: Duration::from_secs(60),
        },
    );
    let mut stalled = Peer::subscribed(&path, "stalled");
    let read: Vec<Arc<AtomicUsize>> = (0..MAX_CONNECTIONS - 1).map(|_| Arc::default()).collect();
    let readers: Vec<_> = read
        .iter()
        .enumerate()
        .map(|(n, count)| {
            let mut peer = Peer::subscribed(&path, &format!("healthy-{n}"));
            let count = Arc::clone(count);
            thread::spawn(move || {
                let mut seen = Vec::new();
                loop {
                    let event = peer.line();
                    if event["event"] == "stopping" {
                        return seen;
                    }
                    seen.push(event["message"].as_str().unwrap()[..6].to_owned());
                    count.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    assert_eq!(board.snapshot().subscribers, MAX_CONNECTIONS);

    // At least QUEUE + 50, and on until the stalled one is dropped, however much its socket
    // buffers hold. Each batch waits until the fifteen have read every event so far, so only
    // the stalled one can fall behind.
    let (mut emitted, mut in_emit) = (0, Duration::ZERO);
    while emitted < QUEUE + 50 || board.snapshot().subscribers == MAX_CONNECTIONS {
        assert!(
            emitted < 16 * QUEUE,
            "the stalled subscriber was never dropped"
        );
        for _ in 0..BATCH {
            let event = burst_event(emitted);
            let started = Instant::now();
            server.hub().emit(&event);
            in_emit += started.elapsed();
            emitted += 1;
        }
        wait_until("the healthy subscribers to read the batch", || {
            read.iter().all(|c| c.load(Ordering::SeqCst) == emitted)
        });
    }
    assert!(
        in_emit < Duration::from_secs(1),
        "emit waited on a client: {in_emit:?} for {emitted} events"
    );
    assert_eq!(board.snapshot().subscribers, MAX_CONNECTIONS - 1);

    server.hub().emit(&DaemonEvent::Stopping {
        reason: StopReason::Signal,
    });
    let want: Vec<String> = (0..emitted).map(|n| format!("{n:06}")).collect();
    for reader in readers {
        assert_eq!(reader.join().unwrap(), want);
    }
    // The stalled one finds what its buffers held, then the end of its connection.
    let got = stalled.rest();
    assert!(
        got.len() < emitted,
        "{} of {emitted} events reached it",
        got.len()
    );
    server.shutdown();
}

#[test]
fn a_line_over_64_kib_closes_the_connection() {
    // D5.2: at most 64 KiB per line, newline left out. A request padded to the limit is read;
    // one byte more closes the connection with no reply, whether its newline never comes,
    // comes with it, or comes later.
    let dir = short_tempdir();
    let (server, _board) = start(dir.path());
    let path = dir.path().join("daemon.sock");
    let padded = |len: usize| {
        let mut line = br#"{"id":1,"op":"status"}"#.to_vec();
        line.resize(len, b' ');
        line
    };
    let mut at_limit = Peer::greeted(&path);
    let mut line = padded(MAX_LINE);
    line.push(b'\n');
    at_limit.send_bytes(&line).unwrap();
    assert_eq!(at_limit.line()["id"], 1);
    let mut over = Peer::greeted(&path);
    // The server may close before the last byte is taken; the write's error is no matter.
    let _ = over.send_bytes(&padded(MAX_LINE + 1));
    assert!(over.rest().is_empty());
    // Terminated, with its newline in the same write or in a later one: closed too, with no
    // reply. A whole line goes through `parse_request`, which closes on one byte over.
    let mut terminated = padded(MAX_LINE + 1);
    terminated.push(b'\n');
    let mut same = Peer::greeted(&path);
    let _ = same.send_bytes(&terminated);
    assert!(same.rest().is_empty());
    let mut later = Peer::greeted(&path);
    let _ = later.send_bytes(&padded(MAX_LINE + 1));
    thread::sleep(Duration::from_millis(200));
    let _ = later.send_bytes(b"\n");
    assert!(later.rest().is_empty());
    server.shutdown();
}

#[test]
fn a_line_that_is_not_a_json_object_closes_the_connection() {
    // D5.2: JSON that is not an object, a line that is not JSON, and one that is not UTF-8.
    let dir = short_tempdir();
    let (server, _board) = start(dir.path());
    let path = dir.path().join("daemon.sock");
    let lines: [&[u8]; 5] = [
        b"[1,2]\n",
        b"\"status\"\n",
        b"not json\n",
        b"\xff\xfe\n",
        b"\n",
    ];
    for line in lines {
        let mut peer = Peer::greeted(&path);
        peer.send_bytes(line).unwrap();
        assert!(peer.rest().is_empty(), "{}", String::from_utf8_lossy(line));
    }
    server.shutdown();
}

#[test]
fn the_seventeenth_connection_is_closed_before_the_greeting() {
    // D5.1: at most sixteen connections. The next is closed at once, and a place given back
    // goes to the next connection.
    let dir = short_tempdir();
    let (server, _board) = start(dir.path());
    let path = dir.path().join("daemon.sock");
    let mut peers: Vec<Peer> = (0..MAX_CONNECTIONS).map(|_| Peer::greeted(&path)).collect();
    assert!(
        Peer::connect(&path).rest().is_empty(),
        "the seventeenth was greeted"
    );
    peers.pop();
    wait_until("a new connection greeted in the freed place", || {
        Peer::connect(&path).next_line().is_some()
    });
    server.shutdown();
}

#[test]
fn an_idle_connection_is_closed_unless_it_subscribed() {
    // D5.2: a connection that has not subscribed and sends nothing for the idle time is
    // closed. One that keeps sending stays, and so does a subscribed one.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    // 300 ms idle against a request every 100 ms: three times the margin a loaded machine
    // needs.
    let (server, _board) = start_with(
        dir.path(),
        Limits {
            idle: Duration::from_millis(300),
            write_timeout: WRITE_TIMEOUT,
        },
    );
    let mut quiet = Peer::greeted(&path);
    let mut talking = Peer::greeted(&path);
    let mut listening = Peer::subscribed(&path, "l");
    let quiet = thread::spawn(move || {
        let started = Instant::now();
        let rest = quiet.rest();
        (started.elapsed(), rest)
    });
    for n in 0..8 {
        thread::sleep(Duration::from_millis(100));
        talking.send(&format!(r#"{{"id":{n},"op":"status"}}"#));
        assert_eq!(talking.line()["id"], n);
    }
    let (after, rest) = quiet.join().unwrap();
    assert!(rest.is_empty());
    assert!(
        after >= Duration::from_millis(250) && after < Duration::from_secs(2),
        "{after:?}"
    );
    server.hub().emit(&DaemonEvent::Changed);
    assert_eq!(listening.line()["event"], "changed");
    server.shutdown();
}

#[test]
fn a_leftover_file_is_replaced_and_the_socket_is_0600_in_a_0700_directory() {
    // D5.1: a missing parent is created 0700, and the socket 0600 through the umask, never
    // chmod'ed. A leftover file at the path, a regular one or a dead socket, is replaced.
    let dir = short_tempdir();
    let path = dir.path().join("data/tagteam/daemon.sock");
    let mode = |p: &Path| fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
    let is_socket = |p: &Path| fs::symlink_metadata(p).unwrap().file_type().is_socket();
    let bind = || Server::bind(&path, Path::new(HOME), board(), Cancel::new()).unwrap();

    let server = bind();
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert!(is_socket(&path));
    assert_eq!(mode(&path), 0o600);
    server.shutdown();

    fs::write(&path, "left behind").unwrap();
    let server = bind();
    assert!(is_socket(&path));
    Peer::greeted(&path);
    server.shutdown();

    drop(UnixListener::bind(&path).unwrap());
    assert!(path.exists(), "a dropped listener leaves its file");
    let server = bind();
    Peer::greeted(&path);
    assert_eq!(mode(&path), 0o600);
    server.shutdown();
}

#[test]
fn a_path_too_long_for_a_socket_address_is_refused() {
    // D5.1: `sun_path` bounds the path. One byte over is `TooLong`, with nothing created; the
    // longest that fits binds.
    let dir = short_tempdir();
    let room = MAX_SOCKET_PATH - dir.path().as_os_str().len() - 1;
    let fits = dir.path().join("s".repeat(room));
    let over = dir.path().join("s".repeat(room + 1));
    assert_eq!(fits.as_os_str().len(), MAX_SOCKET_PATH);
    let Err(BindError::TooLong(refused)) =
        Server::bind(&over, Path::new(HOME), board(), Cancel::new())
    else {
        panic!(
            "a path of {} bytes was not refused as too long",
            MAX_SOCKET_PATH + 1
        );
    };
    assert_eq!(refused, over);
    assert!(!over.exists());
    let server = Server::bind(&fits, Path::new(HOME), board(), Cancel::new()).unwrap();
    Peer::greeted(&fits);
    server.shutdown();
}

#[test]
fn shutdown_closes_connections_and_removes_the_socket_file() {
    // D4.6: every subscriber gets what was emitted before the shutdown, `stopping` last; then
    // every connection ends, the file is gone, and a connect finds nothing.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let (server, board) = start(dir.path());
    let mut subscriber = Peer::subscribed(&path, "s");
    let mut idle = Peer::greeted(&path);
    server.hub().emit(&DaemonEvent::Stopping {
        reason: StopReason::Signal,
    });
    server.shutdown();
    assert_eq!(subscriber.line()["event"], "stopping");
    assert!(subscriber.next_line().is_none());
    assert!(idle.next_line().is_none());
    assert!(!path.exists());
    let refused = UnixStream::connect(&path).unwrap_err();
    assert_eq!(refused.kind(), io::ErrorKind::NotFound);
    assert_eq!(board.snapshot().subscribers, 0);
}

#[test]
fn shutdown_leaves_a_socket_another_daemon_bound() {
    // A daemon that took `daemon.lock` once this one let it go may have replaced the socket
    // already. Neither this one's `stopping` nor its shutdown may remove it.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let (server, _board) = start(dir.path());
    fs::remove_file(&path).unwrap();
    let successor = UnixListener::bind(&path).unwrap();
    server.hub().emit(&DaemonEvent::Stopping {
        reason: StopReason::Signal,
    });
    assert!(path.exists(), "`stopping` removed the successor's socket");
    server.shutdown();
    assert!(path.exists(), "the successor's socket was removed");
    drop(successor);
}

#[test]
fn stopping_removes_the_socket_at_once_and_subscribers_still_read_it() {
    // D4.6, Decision 9: `serve` emits `stopping` while it still holds `daemon.lock`. The hub
    // removes the socket file then, so no successor's socket can be the one removed, and a new
    // connect fails at once. A subscriber still reads `stopping`, and its connection is served
    // until `shutdown`.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let (server, _board) = start(dir.path());
    let mut subscriber = Peer::subscribed(&path, "s");
    server.hub().emit(&DaemonEvent::Stopping {
        reason: StopReason::Signal,
    });
    assert!(!path.exists(), "the socket file outlived `stopping`");
    let refused = UnixStream::connect(&path).unwrap_err();
    assert_eq!(refused.kind(), io::ErrorKind::NotFound);
    assert_eq!(subscriber.line()["event"], "stopping");
    subscriber.send(r#"{"id":3,"op":"status"}"#);
    assert_eq!(
        subscriber.line()["id"],
        3,
        "an open connection is still answered"
    );
    server.shutdown();
    assert!(subscriber.next_line().is_none());
    assert!(!path.exists());
}

#[test]
fn a_signal_stops_new_connections_but_subscribers_still_hear_stopping() {
    // D4.6, Review Focus 1: on the signal the accept thread goes and drops the listener, so
    // a new connect is refused at once. Open connections are still served until `shutdown`,
    // which comes after `serve` has emitted `stopping`.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let cancel = Cancel::new();
    let server = Server::bind(&path, Path::new(HOME), board(), cancel.clone()).unwrap();
    let mut subscriber = Peer::subscribed(&path, "s");
    cancel.request(libc::SIGTERM);
    wait_until(
        "a new connect refused",
        || matches!(UnixStream::connect(&path), Err(e) if e.kind() == io::ErrorKind::ConnectionRefused),
    );
    assert!(path.exists(), "the file stays until `stopping`");
    subscriber.send(r#"{"id":2,"op":"status"}"#);
    assert_eq!(subscriber.line()["id"], 2);
    server.hub().emit(&DaemonEvent::Stopping {
        reason: StopReason::Signal,
    });
    assert_eq!(subscriber.line()["event"], "stopping");
    assert!(!path.exists(), "`stopping` removed the file");
    server.shutdown();
    assert!(subscriber.next_line().is_none());
}

#[test]
fn peer_uid_names_this_process_for_its_own_connection() {
    // Decision 3. A connection from another user cannot be made without root; `admit`
    // compares this value with `geteuid()` and closes on any other.
    let (a, _b) = UnixStream::pair().unwrap();
    // SAFETY: geteuid has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    assert_eq!(peer_uid(&a).unwrap(), euid);
}

#[test]
fn the_hub_can_be_shared_between_threads() {
    // The scheduler emits on the main thread while connection threads subscribe.
    fn shared<T: Send + Sync>() {}
    shared::<Hub>();
}
