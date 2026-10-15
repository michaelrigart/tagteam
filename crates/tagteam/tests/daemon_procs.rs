//! The daemon in real processes (D14 "Concurrency" and "Exits", §15.2): a daemon beside
//! `tagteam auto`, two daemons, a CLI switch reaching a socket subscriber, the hourly usage
//! budget across processes, and the exits on an upgrade and on a signal. Every child is drained
//! while it runs and killed if the test fails (`Guarded`). Roots are short, so the socket fits
//! `sun_path` (D5.1). Needs `--features test-support`, and an environment that may bind and
//! connect Unix sockets and write under `/tmp`: outside an agent's sandbox.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use common::{
    Guarded, cmd, login_with_token, now_epoch_s, record_reading, seed_home, short_tempdir, std_cmd,
    two_fresh_accounts, usage_window,
};
use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, ProviderId, Window, WindowKind};
use tagteam_daemon::client::{Client, Connect, connect_at};
use tagteam_daemon::protocol::{Event, Status};
use tagteam_engine::daemon::{daemon_lock_path, daemon_socket_path, read_daemon_record};
use tagteam_engine::store::{Eligibility, Reserve, SendGrant, Store};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Env, FileKeychain, FlockGuard, Read};

/// How long a binary may take to reach a state: a debug build, with tests in parallel.
const START: Duration = Duration::from_secs(20);
/// How long a stop may take: the scheduler's next slice, then the socket and the locks.
const STOP: Duration = Duration::from_secs(10);
const USAGE: &str = "/api/oauth/usage";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn cc() -> ProviderId {
    ProviderId::new(CLAUDE_CODE)
}

fn store(root: &Path) -> Store {
    Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
}

fn config_set(root: &Path, key: &str, value: &str) {
    cmd(root)
        .args(["config", "set", key, value])
        .assert()
        .success();
}

/// A reading taken at `now`: 5h at `five` and 7d at `seven` (as auto_cli.rs's).
fn reading(now: i64, five: f64, seven: f64) -> Vec<Window> {
    vec![
        usage_window(
            "5h",
            "5h",
            WindowKind::Short,
            five,
            Some(now + 9_630),
            Some(18_000),
        ),
        usage_window(
            "7d",
            "7d",
            WindowKind::Long,
            seven,
            Some(now + 291_630),
            Some(604_800),
        ),
    ]
}

/// `a@x.co` at position 1 and `b@x.co` at position 2 (live), both read just now and below the
/// threshold, so a tick stays and its scheduled collection sends nothing. Notifications are
/// off, so no daemon here starts `osascript` or `notify-send`.
fn staying(root: &Path) -> (String, String) {
    let (a, b) = two_fresh_accounts(root);
    let now = now_epoch_s();
    record_reading(root, &a, now, &reading(now, 10.0, 20.0));
    record_reading(root, &b, now, &reading(now, 10.0, 60.0));
    config_set(root, "notify.enabled", "false");
    (a, b)
}

/// `tagteam daemon <args>` with stdin closed: never a terminal, as under a service, so it runs
/// no Keychain lock check (D4.1 step 3).
fn daemon_cmd(root: &Path, args: &[&str]) -> std::process::Command {
    let mut c = std_cmd(root);
    c.arg("daemon").args(args).stdin(Stdio::null());
    c
}

/// `tagteam <args>`, run to its end.
fn run(root: &Path, args: &[&str]) -> Output {
    let mut c = std_cmd(root);
    c.args(args).stdin(Stdio::null());
    Guarded::spawn(c).finish(START)
}

/// Every complete line of `out` that is a JSON object. A line still being written, and any
/// line that is not JSON, is left out.
fn json_lines(out: &str) -> Vec<Value> {
    let complete = out.rfind('\n').map_or("", |i| &out[..=i]);
    complete
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(Value::is_object)
        .collect()
}

fn has_event(out: &str, want: impl Fn(&Value) -> bool) -> bool {
    json_lines(out).iter().any(want)
}

/// Claude Code's provider job reported mode `mode` (D5.5's `mode` event).
fn mode_event(mode: &'static str) -> impl Fn(&str) -> bool {
    move |out: &str| {
        has_event(out, |e| {
            e["event"] == "mode" && e["provider"] == CLAUDE_CODE && e["mode"] == mode
        })
    }
}

fn engine_lock(root: &Path) -> PathBuf {
    Env::for_test(root)
        .data_dir()
        .join("locks/autoswitch-claude-code.lock")
}

/// The pid the engine lock's record names (§11.1). None for no file, an empty one, or one
/// being rewritten at this instant.
fn engine_holder(root: &Path) -> Option<u32> {
    let bytes = fs::read(engine_lock(root)).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    u32::try_from(v["pid"].as_u64()?).ok()
}

/// The pid `daemon.lock`'s record names (D4.1). None for no record, or one being rewritten.
fn daemon_holder(root: &Path) -> Option<u32> {
    match read_daemon_record(&daemon_lock_path(&Env::for_test(root))) {
        Read::Present(r) => Some(r.pid),
        Read::Absent | Read::Unreadable(_) => None,
    }
}

fn socket(root: &Path) -> PathBuf {
    daemon_socket_path(&Env::for_test(root))
}

/// Polls every 20 ms until `ready` holds. Fails the test after `within`.
fn wait_until(within: Duration, what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + within;
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "never got to {what} within {within:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// D4.6 after the daemon has exited: no socket file; the engine lock's record emptied; and
/// `daemon.lock` with no record. Both locks are free.
fn assert_stopped_cleanly(root: &Path) {
    assert!(!socket(root).exists(), "the socket file was left behind");
    let record = fs::read(engine_lock(root)).unwrap_or_default();
    assert!(
        record.is_empty(),
        "the engine lock's record: {}",
        text(&record)
    );
    let daemon_lock = daemon_lock_path(&Env::for_test(root));
    assert!(
        matches!(read_daemon_record(&daemon_lock), Read::Absent),
        "daemon.lock's record: {:?}",
        read_daemon_record(&daemon_lock)
    );
    for lock in [engine_lock(root), daemon_lock] {
        assert!(
            FlockGuard::try_lock(&lock).unwrap().is_some(),
            "{} is still held",
            lock.display()
        );
    }
}

/// A connection to the daemon at `sock`. `NoDaemon` while it is still starting is retried
/// (Review Focus 1).
fn connected(sock: &Path) -> Client {
    let deadline = Instant::now() + START;
    loop {
        match connect_at(sock).unwrap() {
            Connect::Connected(c) => return c,
            Connect::NoDaemon => {
                assert!(
                    Instant::now() < deadline,
                    "no daemon answered at {}",
                    sock.display()
                );
                thread::sleep(Duration::from_millis(50));
            }
            Connect::Mismatch { server, client } => {
                panic!("the daemon speaks protocol {server}, this client {client}")
            }
        }
    }
}

fn status(sock: &Path) -> Status {
    connected(sock).status().unwrap()
}

/// Subscribes at `sock` and forwards each event on a thread of its own, so a test can wait for
/// one with a deadline. The thread ends when the daemon closes the connection.
fn subscribe(sock: &Path) -> Receiver<Event> {
    let events = connected(sock).subscribe().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for e in events {
            let Ok(e) = e else { break };
            if tx.send(e).is_err() {
                break;
            }
        }
    });
    rx
}

/// The first event from `rx` that `want` accepts, within `within`.
fn next_event(
    rx: &Receiver<Event>,
    within: Duration,
    what: &str,
    want: impl Fn(&Event) -> bool,
) -> Event {
    let deadline = Instant::now() + within;
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(e) if want(&e) => return e,
            Ok(_) => {}
            Err(e) => panic!("no {what} within {within:?}: {e}"),
        }
    }
}

#[test]
fn while_the_daemon_drives_a_provider_auto_reports_engine_running_and_names_it() {
    // D14, D8.3 (Task 6), §11.1: one engine per provider per machine. The daemon holds the
    // engine lock, so `auto --once` reports `engine-running` and a loop refuses, naming the
    // daemon; neither ticks. Pausing then hands the provider over (D4.3, D7; plan A Task 8).
    let d = short_tempdir();
    let root = d.path();
    staying(root);
    let mut daemon = Guarded::spawn(daemon_cmd(root, &["--json"]));
    let pid = daemon.pid();
    daemon.wait_for(START, "auto mode", mode_event("auto"));
    wait_until(START, "the engine lock naming the daemon", || {
        engine_holder(root) == Some(pid)
    });

    let out = run(root, &["auto", "--once", "--json"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    let events = json_lines(&text(&out.stdout));
    assert_eq!(events.len(), 1, "no tick, so no poll: {events:?}");
    assert_eq!(
        (&events[0]["event"], &events[0]["reason"]),
        (&json!("no-switch"), &json!("engine-running"))
    );

    let out = run(root, &["auto"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stdout, b"");
    let err = text(&out.stderr);
    assert!(
        err.contains(&format!("the daemon, pid {pid}"))
            && err.contains("tagteam config set daemon.autoswitch false"),
        "{err}"
    );
    let out = run(root, &["auto", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["type"], "engine-running", "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&format!("the daemon, pid {pid}")),
        "{v}"
    );
    assert_eq!(
        engine_holder(root),
        Some(pid),
        "the daemon kept the lock throughout"
    );

    // Paused, the daemon only collects. It lets the lock go, empties its record, and a tick in
    // another process now runs.
    config_set(root, "daemon.autoswitch", "false");
    daemon.wait_for(STOP, "collect mode", mode_event("collect"));
    wait_until(STOP, "the engine lock's record emptied", || {
        fs::read(engine_lock(root)).unwrap_or_default().is_empty()
    });
    let out = run(root, &["auto", "--once", "--json"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(
        has_event(&text(&out.stdout), |e| {
            e["event"] == "no-switch" && e["reason"] == "below-threshold"
        }),
        "{}",
        text(&out.stdout)
    );

    daemon.signal(libc::SIGTERM);
    let out = daemon.finish(STOP);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

#[test]
fn a_daemon_started_beside_a_running_auto_waits_for_its_engine_lock() {
    // D14, D4.3 (plan A Task 8), D5.4 (Task 1): the other way round. `auto` is the engine; the
    // daemon waits, names it in its event and its status, and never ticks.
    let d = short_tempdir();
    let root = d.path();
    staying(root);
    let mut c = std_cmd(root);
    c.args(["auto", "--json"]).stdin(Stdio::null());
    let mut auto = Guarded::spawn(c);
    let auto_pid = auto.pid();
    auto.wait_for(START, "its first tick", |out| {
        out.contains(r#""event":"no-switch""#)
    });

    let mut daemon = Guarded::spawn(daemon_cmd(root, &["--json"]));
    daemon.wait_for(START, "waiting for the engine lock", |out| {
        has_event(out, |e| {
            e["event"] == "mode"
                && e["mode"] == "waiting"
                && e["waitingFor"] == "engine-lock"
                && e["holderPid"] == auto_pid
        })
    });
    let s = status(&socket(root));
    let p = &s.providers[0];
    assert_eq!(
        (
            p.provider.as_str(),
            p.mode.as_str(),
            p.waiting_for.as_deref(),
            p.holder_pid,
            p.autoswitch
        ),
        (
            "claude-code",
            "waiting",
            Some("engine-lock"),
            Some(auto_pid),
            true
        )
    );
    thread::sleep(Duration::from_secs(2));
    assert!(
        !has_event(&daemon.stdout(), |e| matches!(
            e["event"].as_str(),
            Some("poll" | "switch" | "no-switch")
        )),
        "the waiting daemon ticked: {}",
        daemon.stdout()
    );
    assert_eq!(engine_holder(root), Some(auto_pid));

    daemon.signal(libc::SIGTERM);
    assert_eq!(daemon.finish(STOP).status.code(), Some(0));
    auto.signal(libc::SIGTERM);
    let out = auto.finish(STOP);
    assert_eq!(out.status.code(), Some(0));
    let polls = json_lines(&text(&out.stdout))
        .iter()
        .filter(|e| e["event"] == "poll")
        .count();
    assert_eq!(polls, 1, "auto ticked once, and nothing else ticked");
}

#[test]
fn a_second_daemon_waits_and_takes_over_when_the_first_stops() {
    // D4.1 step 2, D14 (plan A Task 12): one daemon per data directory. The second says whom it
    // waits for, retries every 5 s, and takes over the lock, the socket and the engine once the
    // first has stopped.
    let d = short_tempdir();
    let root = d.path();
    staying(root);
    let mut first = Guarded::spawn(daemon_cmd(root, &["--json"]));
    let first_pid = first.pid();
    first.wait_for(START, "auto mode", mode_event("auto"));
    assert_eq!(daemon_holder(root), Some(first_pid));

    let mut second = Guarded::spawn(daemon_cmd(root, &[]));
    let second_pid = second.pid();
    let notice = format!("a daemon already runs (pid {first_pid}); waiting for it to stop");
    second.wait_for_output(START, "the waiting notice", |out, err| {
        out.contains(&notice) || err.contains(&notice)
    });
    thread::sleep(Duration::from_secs(1));
    assert!(second.is_running());
    assert_eq!(
        daemon_holder(root),
        Some(first_pid),
        "the waiting daemon wrote no record"
    );
    assert_eq!(
        status(&socket(root)).daemon.pid,
        first_pid,
        "the socket is still the first's"
    );

    first.signal(libc::SIGTERM);
    assert_eq!(first.finish(STOP).status.code(), Some(0));
    wait_until(
        Duration::from_secs(15),
        "the second daemon's record",
        || daemon_holder(root) == Some(second_pid),
    );
    wait_until(START, "the engine lock naming the second daemon", || {
        engine_holder(root) == Some(second_pid)
    });
    assert_eq!(status(&socket(root)).daemon.pid, second_pid);

    second.signal(libc::SIGTERM);
    assert_eq!(second.finish(STOP).status.code(), Some(0));
    assert_stopped_cleanly(root);
}

#[test]
fn a_cli_switch_while_the_daemon_runs_reaches_a_subscriber_as_changed() {
    // D4.4 (plan A Task 10), D5.5 (Task 2), D14: another process's commit reaches subscribers
    // as `changed`, within the store watch's 2 s period.
    let d = short_tempdir();
    let root = d.path();
    staying(root);
    let mut daemon = Guarded::spawn(daemon_cmd(root, &["--json"]));
    daemon.wait_for(START, "its first tick", |out| {
        out.contains(r#""event":"no-switch""#)
    });
    let events = subscribe(&socket(root));
    // Whatever the daemon's start produced has arrived after more than one watch period.
    thread::sleep(Duration::from_millis(2_500));
    while events.try_recv().is_ok() {}

    let out = run(root, &["switch", "1", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    next_event(
        &events,
        Duration::from_secs(5),
        "changed after the switch",
        |e| matches!(e, Event::Changed),
    );

    daemon.signal(libc::SIGTERM);
    assert_eq!(daemon.finish(STOP).status.code(), Some(0));
}

/// The recorded usage reply, its windows' resets moved to fixed distances from `now` (as
/// usage_cli.rs's).
fn usage_reply(now: i64) -> MockReply {
    let recorded: Value = serde_json::from_str(include_str!(
        "../../tagteam-cc/tests/fixtures/endpoints/usage-200.json"
    ))
    .unwrap();
    let mut body = recorded["body"].clone();
    body["five_hour"]["resets_at"] = json!(format_iso8601(now + 9_630));
    body["seven_day"]["resets_at"] = json!(format_iso8601(now + 291_630));
    body["limits"][2]["resets_at"] = json!(format_iso8601(now + 291_630));
    MockReply::Json { status: 200, body }
}

/// `a@x.co` at position 1 and `b@x.co` at position 2 (live), each with its own access token, so
/// the server's log tells their identities apart. Neither has a reading.
fn two_bearers(root: &Path) -> (AccountId, AccountId) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login_with_token(&env, &kc, "a@x.co", "", "at-a", "rt-a");
    cmd(root).arg("add").assert().success();
    login_with_token(&env, &kc, "b@x.co", "", "at-b", "rt-b");
    cmd(root).arg("add").assert().success();
    let rows = store(root).accounts(&cc()).unwrap();
    (rows[0].id.clone(), rows[1].id.clone())
}

/// Spends `n` of `id`'s identity's hourly slots (§8.6) at `at_s`, as fetches that never
/// recorded would: one reservation, then `n − 1` further sends under it. With `at_s` more
/// than 90 s ago, its lease has lapsed, so only the budget stands between the account and a
/// fetch.
fn spend(root: &Path, id: &AccountId, n: u32, at_s: i64) {
    let s = store(root);
    let row = s.account(id).unwrap().unwrap();
    let r = match s
        .reserve_usage(
            &row,
            at_s * 1000,
            Eligibility::Scheduled,
            &PollBudget::STANDARD,
        )
        .unwrap()
    {
        Reserve::Reserved(r) => r,
        other => panic!("the first slot is free: {other:?}"),
    };
    for _ in 1..n {
        let grant = s
            .authorize_send(&r, None, None, at_s * 1000, &PollBudget::STANDARD)
            .unwrap();
        assert!(
            matches!(grant, SendGrant::Send(_)),
            "a slot was refused: {grant:?}"
        );
    }
}

/// The usage requests the server logged with `Bearer <at>`, from any process.
fn sent_with(server: &MockServer, at: &str) -> usize {
    let bearer = format!("Bearer {at}");
    server
        .requests()
        .iter()
        .filter(|r| {
            r.path.starts_with(USAGE)
                && r.headers
                    .iter()
                    .any(|(k, v)| k == "authorization" && *v == bearer)
        })
        .count()
}

#[test]
fn the_hourly_budget_holds_with_a_daemon_collecting_beside_list() {
    // §8.6, D12, D14: the budget is per identity, across every process. a's identity has spent
    // its hour and b's has one request left; both accounts are due everywhere. The daemon
    // collects (`daemon.autoswitch` false: D4.3's `collect` row, plan A Task 8), then `list`
    // runs again and again, one after another and four at once. Exactly one usage request
    // leaves, for b.
    let d = short_tempdir();
    let root = d.path();
    let (a, b) = two_bearers(root);
    let spent_at = now_epoch_s() - 100;
    spend(root, &a, PollBudget::STANDARD.hourly_requests, spent_at);
    spend(root, &b, PollBudget::STANDARD.hourly_requests - 1, spent_at);
    config_set(root, "notify.enabled", "false");
    config_set(root, "daemon.autoswitch", "false");
    let server = MockServer::start();
    server.on("GET", USAGE, usage_reply(now_epoch_s()));

    let mut c = daemon_cmd(root, &["--json"]);
    c.env("TAGTEAM_TEST_API_BASE", server.base_url());
    let mut daemon = Guarded::spawn(c);
    daemon.wait_for(START, "a collect round that read b", |out| {
        has_event(out, |e| {
            e["event"] == "usage"
                && e["provider"] == CLAUDE_CODE
                && e["updated"]
                    .as_array()
                    .is_some_and(|u| u.contains(&json!(2)))
        })
    });

    let list = |args: &[&str]| -> Output {
        let mut c = std_cmd(root);
        c.env("TAGTEAM_TEST_API_BASE", server.base_url())
            .args(args)
            .stdin(Stdio::null());
        Guarded::spawn(c).finish(START)
    };
    for _ in 0..3 {
        let out = list(&["list"]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    let together: Vec<Output> = thread::scope(|s| {
        let runs: Vec<_> = (0..4).map(|_| s.spawn(|| list(&["list"]))).collect();
        runs.into_iter().map(|r| r.join().unwrap()).collect()
    });
    for out in &together {
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    }
    let v: Value = serde_json::from_slice(&list(&["list", "--json"]).stdout).unwrap();

    daemon.signal(libc::SIGTERM);
    assert_eq!(daemon.finish(STOP).status.code(), Some(0));

    assert_eq!(
        sent_with(&server, "at-a"),
        0,
        "a's identity had spent its hour"
    );
    assert_eq!(
        sent_with(&server, "at-b"),
        1,
        "b's identity had one request left, and the daemon sent it"
    );
    assert_eq!(server.hits("GET", USAGE), 1, "nothing else was sent");
    assert_eq!(v["accounts"][0]["usageError"], "over-budget", "{v}");
    let since = now_epoch_s() - PollBudget::STANDARD.count_window_s;
    let s = store(root);
    for id in [&a, &b] {
        let key = s.account(id).unwrap().unwrap().identity_key;
        assert_eq!(
            s.usage_request_count(&cc(), &key, since).unwrap(),
            PollBudget::STANDARD.hourly_requests,
            "{key:?}: the hour is full, never over"
        );
    }
}

#[test]
fn a_replaced_program_file_stops_the_daemon_with_exit_75() {
    // D4.5, D8.1, D14 (plan A Tasks 10 and 12): the file the service starts was replaced, as an
    // upgrade replaces it, so the daemon stops as it does on a signal and exits 75, for the
    // service to start the new one. Decision 7's hooks point the watch at a scratch file and
    // check it every 200 ms.
    for json in [true, false] {
        let d = short_tempdir();
        let root = d.path();
        staying(root);
        let program = root.join("bin/tagteam");
        fs::create_dir_all(program.parent().unwrap()).unwrap();
        fs::write(&program, "the installed tagteam\n").unwrap();
        let args: &[&str] = if json { &["--json"] } else { &[] };
        let mut c = daemon_cmd(root, args);
        c.env("TAGTEAM_TEST_UPGRADE_WATCH_MS", "200")
            .env("TAGTEAM_TEST_DAEMON_PROGRAM", &program);
        let mut daemon = Guarded::spawn(c);
        let pid = daemon.pid();
        wait_until(START, "the engine lock naming the daemon", || {
            engine_holder(root) == Some(pid)
        });
        thread::sleep(Duration::from_secs(1)); // five watch periods with the file unchanged
        assert!(daemon.is_running(), "it stopped before any upgrade");
        // A new file renamed over the path, as a package manager installs one: another inode,
        // another size.
        let staged = root.join("bin/.tagteam.new");
        fs::write(&staged, "a newer tagteam, a little longer\n").unwrap();
        fs::rename(&staged, &program).unwrap();
        let out = daemon.finish(STOP);
        assert_eq!(out.status.code(), Some(75), "{}", text(&out.stderr));
        if json {
            let events = json_lines(&text(&out.stdout));
            let last = events.last().unwrap();
            assert_eq!(
                (&last["event"], &last["reason"]),
                (&json!("stopping"), &json!("upgrade")),
                "{events:?}"
            );
            assert_eq!(
                events.iter().filter(|e| e["event"] == "stopping").count(),
                1
            );
        } else {
            let all = text(&out.stdout) + &text(&out.stderr);
            assert!(
                all.contains("tagteam was upgraded; start the daemon again"),
                "{all}"
            );
        }
        assert_stopped_cleanly(root);
    }
}

#[test]
fn a_program_replaced_while_a_second_daemon_waits_is_an_upgrade_at_its_first_watch_run() {
    // D4.5, plan A Decision 17: `tagteam daemon` stamps its program first thing, before the
    // `daemon.lock` wait. A tagteam installed while a second daemon waits for the first to stop
    // is an upgrade at the second's first upgrade-watch run once it serves (exit 75), never its
    // own baseline. Decision 7's hooks point the second's watch at a scratch file and check it
    // every 200 ms; the first daemon has no hooks.
    let d = short_tempdir();
    let root = d.path();
    staying(root);
    let program = root.join("bin/tagteam");
    fs::create_dir_all(program.parent().unwrap()).unwrap();
    fs::write(&program, "the installed tagteam\n").unwrap();
    let mut first = Guarded::spawn(daemon_cmd(root, &["--json"]));
    let first_pid = first.pid();
    first.wait_for(START, "auto mode", mode_event("auto"));

    let mut c = daemon_cmd(root, &["--json"]);
    c.env("TAGTEAM_TEST_UPGRADE_WATCH_MS", "200")
        .env("TAGTEAM_TEST_DAEMON_PROGRAM", &program);
    let mut second = Guarded::spawn(c);
    // The notice comes from the `daemon.lock` wait, so the stamp has been taken by now.
    let notice = format!("a daemon already runs (pid {first_pid}); waiting for it to stop");
    second.wait_for_output(START, "the waiting notice", |out, err| {
        out.contains(&notice) || err.contains(&notice)
    });
    // A newer tagteam is installed while the second waits: a new file renamed over the path,
    // another inode and another size.
    let staged = root.join("bin/.tagteam.new");
    fs::write(&staged, "a newer tagteam, a little longer\n").unwrap();
    fs::rename(&staged, &program).unwrap();
    assert!(second.is_running(), "it stopped while it waited");
    assert_eq!(
        daemon_holder(root),
        Some(first_pid),
        "the second never held daemon.lock"
    );

    first.signal(libc::SIGTERM);
    assert_eq!(first.finish(STOP).status.code(), Some(0));
    // The lock is tried again every 5 s; then the first watch run comes as the daemon serves.
    let out = second.finish(START);
    assert_eq!(out.status.code(), Some(75), "{}", text(&out.stderr));
    let events = json_lines(&text(&out.stdout));
    let last = events.last().unwrap();
    assert_eq!(
        (&last["event"], &last["reason"]),
        (&json!("stopping"), &json!("upgrade")),
        "{events:?}"
    );
    assert_eq!(
        events.iter().filter(|e| e["event"] == "stopping").count(),
        1
    );
    assert_stopped_cleanly(root);
}

#[test]
fn a_signal_stops_the_daemon_with_exit_0_and_leaves_no_socket_or_lock_record() {
    // D4.6, D8.1, D14 (plan A Task 12, Task 2): SIGTERM and SIGHUP, as for every command
    // (§14.1). The subscriber hears `stopping` with reason `signal` last, then the connection
    // closes; the socket goes, the engine lock's record is emptied, and `daemon.lock` is free
    // with no record.
    for signal in [libc::SIGTERM, libc::SIGHUP] {
        let d = short_tempdir();
        let root = d.path();
        staying(root);
        let mut daemon = Guarded::spawn(daemon_cmd(root, &["--json"]));
        let pid = daemon.pid();
        daemon.wait_for(START, "auto mode", mode_event("auto"));
        wait_until(START, "the engine lock naming the daemon", || {
            engine_holder(root) == Some(pid)
        });
        let events = subscribe(&socket(root));
        daemon.signal(signal);
        let last = next_event(&events, STOP, "stopping", |e| {
            matches!(e, Event::Stopping { .. })
        });
        assert_eq!(
            last,
            Event::Stopping {
                reason: "signal".into()
            },
            "{signal}"
        );
        assert!(
            matches!(
                events.recv_timeout(STOP),
                Err(RecvTimeoutError::Disconnected)
            ),
            "{signal}: the connection closed after `stopping`"
        );
        let out = daemon.finish(STOP);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{signal}: {}",
            text(&out.stderr)
        );
        let printed = json_lines(&text(&out.stdout));
        let final_event = printed.last().unwrap();
        assert_eq!(
            (&final_event["event"], &final_event["reason"]),
            (&json!("stopping"), &json!("signal")),
            "{signal}"
        );
        assert_stopped_cleanly(root);
    }
}

/// One final assistant line of a transcript (token report §2.1), as tokens.rs's.
fn transcript_line(session: &str, n: u64) -> String {
    json!({
        "type": "assistant", "sessionId": session, "requestId": format!("req_{n:06}"),
        "timestamp": "2026-10-14T20:00:00.000Z", "cwd": "/work/app", "isSidechain": false,
        "message": {"id": format!("msg_{n:06}"), "model": "claude-opus-5-5",
                    "stop_reason": "end_turn",
                    "usage": {"input_tokens": n, "output_tokens": 10,
                              "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}
    })
    .to_string()
}

#[test]
fn a_daemon_keeps_the_token_index_and_reports_its_jobs() {
    // D13, token report §12 (Task 9): the daemon indexes the default home's transcripts, says
    // so with `token-index`, and lists both jobs in `status` (D5.4).
    let d = short_tempdir();
    let root = d.path();
    staying(root);
    let transcript = Env::for_test(root)
        .home
        .join(".claude/projects/-work-app/s1.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(&transcript, format!("{}\n", transcript_line("s1", 1))).unwrap();

    let mut daemon = Guarded::spawn(daemon_cmd(root, &["--json"]));
    daemon.wait_for(START, "a token-index event", |out| {
        has_event(out, |e| {
            e["event"] == "token-index" && e["provider"] == CLAUDE_CODE && e["responses"] == 1
        })
    });
    let s = status(&socket(root));
    for name in ["sessions-watch", "token-index"] {
        let job = s
            .jobs
            .iter()
            .find(|j| j.name == name)
            .unwrap_or_else(|| panic!("{name} is not in status"));
        assert_eq!(job.last_error, None, "{name}");
        assert!(
            job.last_run_at.is_some() && job.next_run_at.is_some(),
            "{name}"
        );
    }

    daemon.signal(libc::SIGTERM);
    assert_eq!(daemon.finish(STOP).status.code(), Some(0));
}
