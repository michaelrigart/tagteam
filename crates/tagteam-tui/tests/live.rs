//! The live thread and the status fetch (spec §8, §11 "Daemon client") against the daemon's own
//! socket server (`tagteam_daemon::server`), and against fake daemons for what the real one
//! does only under load, across versions, or when it hangs: a protocol it does not speak, a
//! dropped subscriber, a `stopping` with the socket left open, a link gone quiet, a `status`
//! that never comes. Run outside a sandbox that refuses `bind` on Unix sockets.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tagteam_core::ProviderId;
use tagteam_daemon::events::{DaemonEvent, DaemonSink, Mode, StopReason};
use tagteam_daemon::protocol::Event;
use tagteam_daemon::server::Server;
use tagteam_daemon::status::{DaemonInfo, NotifyInfo, ProviderStatus, StatusBoard};
use tagteam_engine::tokens::IndexState;
use tagteam_provider::Cancel;
use tagteam_tui::live::{Backoff, Delivery, Live, LiveQueue, NO_DAEMON, fetch_status, spawn};

/// How long anything here may take before it counts as stuck.
const WAIT: Duration = Duration::from_secs(10);
/// 2026-09-21T14:13:20Z.
const T: i64 = 1_790_000_000;

/// A temporary directory short enough for a socket path under it (the daemon's D14): made
/// under `/tmp`, else `$TMPDIR`, and made 0700 after, since a bind elsewhere in this binary
/// may have narrowed the umask at that instant.
fn short_tempdir() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("tt")
        .tempdir_in("/tmp")
        .or_else(|_| tempfile::Builder::new().prefix("tt").tempdir())
        .expect("a temporary directory under /tmp or $TMPDIR");
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

/// A board for a daemon that runs Claude Code's job in `mode`, with `autoswitch`.
fn board(mode: Mode, autoswitch: bool) -> Arc<StatusBoard> {
    let board = StatusBoard::new(
        DaemonInfo {
            pid: 4242,
            version: "20261014.2045.0".into(),
            started_at: T,
            socket: None,
            log: PathBuf::from("/Users/tester/.local/state/tagteam/tagteam.log"),
        },
        NotifyInfo {
            enabled: false,
            backend: None,
            events: vec![],
        },
    );
    board.set_provider(ProviderStatus {
        provider: ProviderId::new("claude-code"),
        mode,
        autoswitch,
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

fn serve(path: &Path) -> Server {
    Server::bind(
        path,
        Path::new("/Users/tester"),
        board(Mode::Auto, true),
        Cancel::new(),
    )
    .unwrap()
}

/// Backoffs short enough for a test.
fn quick(first_ms: u64, most_ms: u64, mismatch_ms: u64) -> Backoff {
    Backoff {
        first: Duration::from_millis(first_ms),
        most: Duration::from_millis(most_ms),
        mismatch: Duration::from_millis(mismatch_ms),
    }
}

/// The live thread on `path`, with a reader that takes its queue at each wake, as the UI
/// thread does, and passes on each thing it took with when it took it.
fn live(path: &Path, backoff: Backoff) -> (Receiver<(Instant, Live)>, Arc<LiveQueue>) {
    let (wake, woken) = mpsc::channel();
    let queue = Arc::new(LiveQueue::new(256, move || wake.send(()).is_ok()));
    spawn(path.to_path_buf(), backoff, Arc::clone(&queue), || {}).unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = Arc::clone(&queue);
    thread::spawn(move || {
        while woken.recv().is_ok() {
            let at = Instant::now();
            for l in reader.take() {
                if tx.send((at, l)).is_err() {
                    return;
                }
            }
        }
    });
    (rx, queue)
}

/// The next thing the live thread passed on.
fn next(rx: &Receiver<(Instant, Live)>) -> (Instant, Live) {
    rx.recv_timeout(WAIT)
        .expect("the live thread passed something on")
}

/// Skips `Down`s until the link comes up again.
fn up_again(rx: &Receiver<(Instant, Live)>, what: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        match next(rx).1 {
            Live::Up { .. } => return,
            Live::Down => assert!(Instant::now() < deadline, "never came up: {what}"),
            other => panic!("{what}: {other:?}"),
        }
    }
}

/// A fake daemon at `path` that serves each connection it accepts with `script`, one at a
/// time, until the test ends.
fn fake(path: &Path, script: impl Fn(UnixStream) + Send + 'static) {
    let listener = UnixListener::bind(path).unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            stream.set_read_timeout(Some(WAIT)).unwrap();
            script(stream);
        }
    });
}

fn hello(protocol: u32) -> String {
    format!(
        "{{\"schemaVersion\":1,\"event\":\"hello\",\"protocol\":{protocol},\"version\":\"20261014.2045.0\",\"pid\":4242,\"startedAt\":\"2026-09-21T14:13:20Z\"}}\n"
    )
}

/// The request a fake daemon reads next, as text.
fn request(stream: &UnixStream) -> String {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    line
}

/// A fake daemon's greeting and its `ok` to the client's subscription (the client's first
/// request has id 1).
fn subscribed(stream: &mut UnixStream) {
    stream.write_all(hello(1).as_bytes()).unwrap();
    request(stream);
    let ok = "{\"schemaVersion\":1,\"id\":1,\"ok\":true,\"result\":{}}\n";
    stream.write_all(ok.as_bytes()).unwrap();
}

#[test]
fn a_daemon_brings_the_link_up_and_its_events_through_in_order() {
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let server = serve(&path);
    let (rx, _queue) = live(&path, Backoff::default());
    assert_eq!(next(&rx).1, Live::Up { link: 1 });
    let cc = ProviderId::new("claude-code");
    server.hub().emit(&DaemonEvent::Changed);
    server.hub().emit(&DaemonEvent::Usage {
        provider: cc.clone(),
        updated: vec![1],
        fetch_errors: Default::default(),
    });
    server.hub().emit(&DaemonEvent::TokenIndex {
        provider: cc,
        state: IndexState {
            changed_at_ms: Some(T * 1000),
            ..IndexState::default()
        },
    });
    assert_eq!(next(&rx).1, Live::Event(Event::Changed));
    assert!(
        matches!(next(&rx).1, Live::Event(Event::Usage { updated, .. }) if updated == [1]),
        "the usage event second"
    );
    match next(&rx).1 {
        Live::Event(Event::TokenIndex { provider, raw }) => {
            assert_eq!(provider, "claude-code");
            assert_eq!(raw["changedAt"], "2026-09-21T14:13:20Z");
        }
        other => panic!("{other:?}"),
    }
    server.shutdown();
}

#[test]
fn with_no_daemon_it_says_so_and_tries_again_less_and_less_often_until_one_answers() {
    // Spec §8: backoff from `first` up to `most`, never a busy loop. The exact waits are
    // `live::tests`; here only what a loaded machine cannot break: every `Down` comes no sooner
    // than the waits before it add up to (the reader stamps late, never early), no more come in
    // the window than the waits allow, and the first daemon to answer brings the link up.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let started = Instant::now();
    let (rx, _queue) = live(&path, quick(20, 160, 1_000));
    let window = started + Duration::from_millis(800);
    let mut downs = vec![next(&rx)];
    while let Some(left) = window.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(got) => downs.push(got),
            Err(_) => break,
        }
    }
    assert!(
        downs.iter().all(|(_, l)| *l == Live::Down),
        "never anything but Down: {downs:?}"
    );
    // 0, 20, 60, 140, 300, 460, 620, 780 ms at the soonest, and 940 ms next.
    assert!(
        downs.len() <= 10,
        "a busy loop: {} Downs in 800 ms",
        downs.len()
    );
    let mut soonest = Duration::ZERO;
    for (i, ((at, _), wait)) in downs
        .iter()
        .zip(
            [0, 20, 40, 80, 160]
                .into_iter()
                .chain(std::iter::repeat(160)),
        )
        .enumerate()
    {
        soonest += Duration::from_millis(wait);
        assert!(
            *at - started >= soonest,
            "Down {i} came after {:?}, the waits make it {soonest:?}",
            *at - started
        );
    }
    let server = serve(&path);
    up_again(&rx, "the daemon that came");
    server.shutdown();
}

#[test]
fn a_daemon_of_another_protocol_is_a_mismatch_tried_again_after_its_own_wait() {
    // Spec §8, D5.6: in case the daemon restarts on this version. The wait is measured between
    // the fake's own accepts, which load cannot move closer together.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let accepts = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&accepts);
    fake(&path, move |mut stream| {
        seen.lock().unwrap().push(Instant::now());
        stream.write_all(hello(2).as_bytes()).unwrap();
    });
    let (rx, _queue) = live(&path, quick(10, 20, 150));
    assert_eq!(next(&rx).1, Live::Mismatch { server: 2 });
    assert_eq!(
        next(&rx).1,
        Live::Mismatch { server: 2 },
        "never Down in between"
    );
    let accepts = accepts.lock().unwrap();
    assert!(accepts[1] - accepts[0] >= Duration::from_millis(150));
}

#[test]
fn stopping_comes_through_then_the_link_goes_down_and_comes_back_with_the_next_daemon() {
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let server = serve(&path);
    let (rx, _queue) = live(&path, quick(20, 1_000, 1_000));
    assert_eq!(next(&rx).1, Live::Up { link: 1 });
    server.hub().emit(&DaemonEvent::Stopping {
        reason: StopReason::Upgrade,
    });
    server.shutdown();
    assert_eq!(
        next(&rx).1,
        Live::Event(Event::Stopping {
            reason: "upgrade".into()
        })
    );
    assert_eq!(next(&rx).1, Live::Down);
    let restarted = serve(&path);
    up_again(&rx, "the next daemon");
    restarted.shutdown();
}

#[test]
fn stopping_ends_the_link_at_once_though_the_daemon_keeps_its_socket_open() {
    // C-R18: a daemon that says `stopping` and then stalls before it closes is not waited for.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    fake(&path, |mut stream| {
        subscribed(&mut stream);
        let stopping = "{\"schemaVersion\":1,\"event\":\"stopping\",\"ts\":\"2026-09-21T14:13:20Z\",\"reason\":\"signal\"}\n";
        stream.write_all(stopping.as_bytes()).unwrap();
        thread::sleep(Duration::from_secs(5));
    });
    let (rx, _queue) = live(&path, quick(50, 100, 1_000));
    assert_eq!(next(&rx).1, Live::Up { link: 1 });
    let (said, stopping) = next(&rx);
    assert_eq!(
        stopping,
        Live::Event(Event::Stopping {
            reason: "signal".into()
        })
    );
    let (down, l) = next(&rx);
    assert_eq!(l, Live::Down);
    assert!(down - said < Duration::from_secs(1), "{:?}", down - said);
}

#[test]
fn a_daemon_that_closes_its_side_ends_the_link() {
    // A half-closed connection: the daemon writes no more, and the link is down at once.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    fake(&path, |mut stream| {
        subscribed(&mut stream);
        stream.shutdown(Shutdown::Write).unwrap();
        thread::sleep(Duration::from_secs(5));
    });
    let (rx, _queue) = live(&path, quick(50, 100, 1_000));
    let (up, l) = next(&rx);
    assert_eq!(l, Live::Up { link: 1 });
    let (down, l) = next(&rx);
    assert_eq!(l, Live::Down);
    assert!(down - up < Duration::from_secs(1), "{:?}", down - up);
}

#[test]
fn end_link_ends_a_link_gone_quiet_and_the_thread_connects_again() {
    // C-R19: the live thread's read waits for as long as the daemon is silent; a daemon that
    // hangs with its socket open is found out by `status` (Task 2), which ends the link here.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    fake(&path, |mut stream| {
        subscribed(&mut stream);
        thread::sleep(Duration::from_secs(5));
    });
    let (rx, queue) = live(&path, quick(50, 100, 1_000));
    assert_eq!(next(&rx).1, Live::Up { link: 1 });
    thread::sleep(Duration::from_millis(100));
    assert!(rx.try_recv().is_err(), "silent, and still up");
    let ended = Instant::now();
    queue.end_link(1);
    let (down, l) = next(&rx);
    assert_eq!(l, Live::Down);
    assert!(down - ended < Duration::from_secs(1), "{:?}", down - ended);
    queue.end_link(1);
}

/// A fake daemon that takes each subscription, then drops the subscriber after the next of
/// `holds` (popped from its end), as the real one drops a subscriber whose queue is full
/// (D5.5: it shuts the socket down). It returns when it accepted each connection.
fn dropping(path: &Path, holds: Vec<Duration>) -> Arc<Mutex<Vec<Instant>>> {
    let holds = Mutex::new(holds);
    let accepts = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&accepts);
    fake(path, move |mut stream| {
        seen.lock().unwrap().push(Instant::now());
        subscribed(&mut stream);
        let hold = holds.lock().unwrap().pop().unwrap_or_default();
        thread::sleep(hold);
        let _ = stream.shutdown(Shutdown::Both);
    });
    accepts
}

#[test]
fn end_link_ends_only_the_link_it_names() {
    // C-R21: the UI may judge a link that the live thread has already replaced (its `status`
    // failed while a suspension held the messages back). Ending that link leaves the new one.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    // The first link is dropped at once; the second holds.
    dropping(&path, vec![Duration::from_secs(5), Duration::ZERO]);
    let (rx, queue) = live(&path, quick(20, 100, 1_000));
    assert_eq!(next(&rx).1, Live::Up { link: 1 });
    assert_eq!(next(&rx).1, Live::Down);
    assert_eq!(next(&rx).1, Live::Up { link: 2 });
    queue.end_link(1);
    thread::sleep(Duration::from_millis(300));
    assert!(rx.try_recv().is_err(), "the second link carries on");
    queue.end_link(2);
    assert_eq!(next(&rx).1, Live::Down);
}

#[test]
fn a_subscriber_the_daemon_drops_is_down_and_reconnects_less_often_while_it_keeps_dropping() {
    // Spec §8: a full queue is a disconnect, and the TUI reconnects with backoff. Links dropped
    // at once double the wait (`live::tests` has the exact waits and the reset after a link that
    // lasted `most`). Here, in what load cannot break: each link is `Up` then `Down`, never a
    // mismatch, and the fake's own accepts are no closer than the hold and the wait between them.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let at_once = Duration::ZERO;
    let long = Duration::from_millis(800);
    // The fourth link lasts well over `most`; the others are dropped at once.
    let accepts = dropping(&path, vec![at_once, long, at_once, at_once, at_once]);
    let (rx, _queue) = live(&path, quick(30, 200, 1_000));
    for link in 1..=5 {
        assert_eq!(next(&rx).1, Live::Up { link }, "link {link} up");
        if link < 5 {
            assert_eq!(next(&rx).1, Live::Down, "link {link} down");
        }
    }
    let accepts = accepts.lock().unwrap();
    let least = [30, 60, 120, 800 + 30].map(Duration::from_millis);
    for (i, least) in least.into_iter().enumerate() {
        let gap = accepts[i + 1] - accepts[i];
        assert!(
            gap >= least,
            "accepts {i} and {}: {gap:?} < {least:?}",
            i + 1
        );
    }
}

#[test]
fn a_full_queue_takes_one_down_for_what_it_held() {
    // C-R17: at most `cap` wait; a full queue keeps a single `Down`, and says so.
    let woken = Arc::new(Mutex::new(0));
    let count = Arc::clone(&woken);
    let queue = LiveQueue::new(3, move || {
        *count.lock().unwrap() += 1;
        true
    });
    assert!(queue.is_empty());
    assert_eq!(queue.push(Live::Up { link: 1 }), Delivery::Taken);
    assert_eq!(queue.push(Live::Event(Event::Changed)), Delivery::Taken);
    assert_eq!(queue.push(Live::Event(Event::Changed)), Delivery::Taken);
    assert_eq!(
        *woken.lock().unwrap(),
        1,
        "woken when it went from empty to not"
    );
    assert_eq!(queue.push(Live::Event(Event::Changed)), Delivery::Full);
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.take(), [Live::Down]);
    assert_eq!(queue.push(Live::Up { link: 1 }), Delivery::Taken);
    assert_eq!(*woken.lock().unwrap(), 2);
    let gone = LiveQueue::new(3, || false);
    assert_eq!(gone.push(Live::Up { link: 1 }), Delivery::Gone);
}

#[test]
fn a_queue_the_tui_closed_is_gone_whatever_it_holds() {
    // The UI quits while something still waits: no later push may see the queue as listened to.
    let queue = LiveQueue::new(3, || true);
    assert_eq!(queue.push(Live::Up { link: 1 }), Delivery::Taken);
    queue.close();
    assert_eq!(queue.push(Live::Event(Event::Changed)), Delivery::Gone);
    assert_eq!(queue.push(Live::Down), Delivery::Gone);
}

#[test]
fn the_thread_ends_once_the_tui_closed_a_queue_that_still_holds_something() {
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let queue = Arc::new(LiveQueue::new(1_000, || true));
    queue.push(Live::Down);
    let thread = spawn(path, quick(10, 10, 10), Arc::clone(&queue), || {}).unwrap();
    queue.close();
    let deadline = Instant::now() + WAIT;
    while !thread.is_finished() {
        assert!(Instant::now() < deadline, "the thread never ended");
        thread::sleep(Duration::from_millis(5));
    }
    thread.join().unwrap();
}

#[test]
fn a_line_the_link_cannot_read_ends_it_with_down() {
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    fake(&path, |mut stream| {
        subscribed(&mut stream);
        stream.write_all(b"this is not json\n").unwrap();
        thread::sleep(Duration::from_secs(5));
    });
    let (rx, _queue) = live(&path, quick(50, 100, 1_000));
    let (up, l) = next(&rx);
    assert_eq!(l, Live::Up { link: 1 });
    let (down, l) = next(&rx);
    assert_eq!(l, Live::Down);
    assert!(down - up < Duration::from_secs(1), "{:?}", down - up);
}

#[test]
fn a_flood_nobody_takes_stays_within_the_queue_and_the_link_is_dropped() {
    // C-R17: while the UI takes nothing (a suspension, §7.3) the backlog never passes the
    // queue's size. The live thread drops the link at the overflow, and the daemon sees it go.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let board = board(Mode::Auto, true);
    let server = Server::bind(
        &path,
        Path::new("/Users/tester"),
        Arc::clone(&board),
        Cancel::new(),
    )
    .unwrap();
    let queue = Arc::new(LiveQueue::new(4, || true));
    spawn(
        path.clone(),
        quick(5_000, 5_000, 5_000),
        Arc::clone(&queue),
        || {},
    )
    .unwrap();
    let deadline = Instant::now() + WAIT;
    while board.snapshot().subscribers == 0 {
        assert!(Instant::now() < deadline, "never subscribed");
        thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..20 {
        server.hub().emit(&DaemonEvent::Changed);
        assert!(queue.len() <= 4, "{}", queue.len());
        thread::sleep(Duration::from_millis(20));
    }
    while board.snapshot().subscribers != 0 {
        assert!(Instant::now() < deadline, "the link was never dropped");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(queue.len() <= 4);
    let taken = queue.take();
    assert_eq!(taken.first(), Some(&Live::Down), "{taken:?}");
    server.shutdown();
}

#[test]
fn the_thread_ends_once_the_tui_no_longer_listens() {
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let queue = Arc::new(LiveQueue::new(4, || false));
    let thread = spawn(path, quick(10, 10, 10), queue, || {}).unwrap();
    let deadline = Instant::now() + WAIT;
    while !thread.is_finished() {
        assert!(Instant::now() < deadline, "the thread never ended");
        thread::sleep(Duration::from_millis(5));
    }
    thread.join().unwrap();
}

#[test]
fn a_panic_on_the_live_thread_calls_on_death() {
    // The TUI treats it as a lane that died (spec §4.3, §10.4).
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let died = Arc::new(AtomicBool::new(false));
    let told = Arc::clone(&died);
    let queue = Arc::new(LiveQueue::new(4, || {
        panic!("the live thread's wake panicked")
    }));
    let thread: JoinHandle<()> = spawn(path, Backoff::default(), queue, move || {
        told.store(true, Ordering::SeqCst)
    })
    .unwrap();
    assert!(thread.join().is_err());
    assert!(died.load(Ordering::SeqCst));
}

#[test]
fn status_comes_over_a_connection_of_its_own_beside_the_link() {
    // Spec §8: `subscribe` keeps the live thread's connection, so `status` opens another.
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let server = Server::bind(
        &path,
        Path::new("/Users/tester"),
        board(Mode::Collect, false),
        Cancel::new(),
    )
    .unwrap();
    let (rx, _queue) = live(&path, Backoff::default());
    assert_eq!(next(&rx).1, Live::Up { link: 1 });
    let status = fetch_status(&path).unwrap();
    assert_eq!(status.providers.len(), 1);
    let p = &status.providers[0];
    assert_eq!((p.mode.as_str(), p.autoswitch), ("collect", false));
    server.hub().emit(&DaemonEvent::Changed);
    assert_eq!(
        next(&rx).1,
        Live::Event(Event::Changed),
        "the link carries on"
    );
    server.shutdown();
}

#[test]
fn status_with_no_daemon_or_another_protocol_fails_at_once() {
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    let started = Instant::now();
    assert_eq!(fetch_status(&path).unwrap_err(), NO_DAEMON);
    fake(&path, |mut stream| {
        stream.write_all(hello(2).as_bytes()).unwrap();
    });
    assert_eq!(
        fetch_status(&path).unwrap_err(),
        "the daemon speaks protocol 2"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_daemon_that_never_answers_status_is_given_up_on_after_2_s() {
    // D5.7's bound: the daemon lane is never held longer, and a shutdown never waits for it
    // (spec §10.2).
    let dir = short_tempdir();
    let path = dir.path().join("daemon.sock");
    fake(&path, |mut stream| {
        stream.write_all(hello(1).as_bytes()).unwrap();
        request(&stream);
        thread::sleep(Duration::from_secs(5));
    });
    let started = Instant::now();
    let error = fetch_status(&path).unwrap_err();
    let took = started.elapsed();
    assert_eq!(error, "the daemon did not answer in time");
    assert!(
        took >= Duration::from_secs(2) && took < Duration::from_secs(4),
        "{took:?}"
    );
}
