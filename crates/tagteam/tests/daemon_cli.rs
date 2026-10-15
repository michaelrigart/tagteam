//! `tagteam daemon` through the real binary (D8.1, D4.1, D4.6): its events, its exits, the run
//! shell's refusal, and a second daemon's wait. Every endpoint stays offline (`std_cmd`'s
//! default), and readings are recorded fresh, so a tick sends nothing. Notifications are off,
//! because the fixture says macOS, where they would run the real `/usr/bin/osascript`. Every
//! run is drained while it runs (`Running`). Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{
    Running, cmd, now_epoch_s, record_reading, std_cmd, two_fresh_accounts, usage_window,
};
use serde_json::{Value, json};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId, WindowKind};
use tagteam_provider::Env;
use tagteam_provider::profile::{ProfileMarker, canonical_profile_path};

const INSIDE: &str = "this command cannot run inside a `tagteam run` session";

/// `a@x.co` at position 1 and `b@x.co` at 2, live. b is at 60 % of its 7d window and a at
/// 20 %, read now, so a tick stays. Notifications are off.
fn staying(root: &Path) -> (String, String) {
    let (a, b) = two_fresh_accounts(root);
    let now = now_epoch_s();
    for (id, seven) in [(&a, 20.0), (&b, 60.0)] {
        let windows = [
            usage_window(
                "5h",
                "5h",
                WindowKind::Short,
                10.0,
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
        ];
        record_reading(root, id, now, &windows);
    }
    cmd(root)
        .args(["config", "set", "notify.enabled", "false"])
        .assert()
        .success();
    (a, b)
}

fn daemon(root: &Path, args: &[&str]) -> std::process::Command {
    let mut c = std_cmd(root);
    c.arg("daemon").args(args);
    c
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn events(bytes: &[u8]) -> Vec<Value> {
    text(bytes)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Each stdout line after its `HH:MM:SS  ` time.
fn untimed(bytes: &[u8]) -> Vec<String> {
    text(bytes)
        .lines()
        .map(|l| {
            let (time, rest) = l.split_at(10);
            assert!(time.ends_with("  ") && time.as_bytes()[2] == b':', "{l}");
            rest.to_owned()
        })
        .collect()
}

fn lock(root: &Path, name: &str) -> PathBuf {
    Env::for_test(root).data_dir().join("locks").join(name)
}

#[test]
fn json_prints_its_events_and_a_signal_stops_it_with_exit_0_and_both_records_emptied() {
    // D8.1, D4.6: SIGTERM and SIGINT each end it at the next cancellation point. `stopping`
    // is the last event and the exit is 0. The records of `daemon.lock` and of the engine lock
    // it held are emptied before the locks go.
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let d = tempfile::tempdir().unwrap();
        staying(d.path());
        let mut running = Running::spawn(daemon(d.path(), &["--json"]));
        running.wait_for(Duration::from_secs(20), "its first tick", |out| {
            out.contains(r#""event":"no-switch""#)
        });
        let record: Value =
            serde_json::from_slice(&fs::read(lock(d.path(), "daemon.lock")).unwrap()).unwrap();
        assert_eq!(record["pid"], json!(running.child.id()), "{signal}");
        assert_eq!(record["version"], json!(env!("CARGO_PKG_VERSION")));
        running.signal(signal);
        let out = running.finish(Duration::from_secs(10));
        assert_eq!(
            out.status.code(),
            Some(0),
            "{signal}: {}",
            text(&out.stderr)
        );
        assert_eq!(text(&out.stderr), "", "{signal}");
        let events = events(&out.stdout);
        let modes: Vec<&Value> = events.iter().filter(|e| e["event"] == "mode").collect();
        assert_eq!(
            (modes[0]["provider"].as_str(), modes[0]["mode"].as_str()),
            (Some("claude-code"), Some("auto")),
            "{signal}"
        );
        let last = events.last().unwrap();
        assert_eq!(
            (last["event"].as_str(), last["reason"].as_str()),
            (Some("stopping"), Some("signal"))
        );
        for name in ["daemon.lock", "autoswitch-claude-code.lock"] {
            assert_eq!(
                fs::read(lock(d.path(), name)).unwrap(),
                b"",
                "{signal}: {name}"
            );
        }
    }
}

#[test]
fn human_output_has_auto_s_tick_lines_and_the_daemon_s_own() {
    // D8.1: `auto`'s line per tick, and the daemon's lines for its start, modes and stop.
    let d = tempfile::tempdir().unwrap();
    staying(d.path());
    let mut running = Running::spawn(daemon(d.path(), &[]));
    running.wait_for(Duration::from_secs(20), "its first tick", |out| {
        out.contains("no switch: below-threshold")
    });
    let pid = running.child.id();
    running.signal(libc::SIGTERM);
    let out = running.finish(Duration::from_secs(10));
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(text(&out.stderr), "");
    let lines = untimed(&out.stdout);
    assert_eq!(
        lines[0],
        format!("daemon started (pid {pid}); Ctrl-C stops it")
    );
    assert!(lines.contains(&"mode auto".to_owned()), "{lines:?}");
    assert!(
        lines.contains(&"#2 b@x.co  60%  no switch: below-threshold".to_owned()),
        "{lines:?}"
    );
    assert_eq!(lines.last().unwrap(), "stopped");
}

#[test]
fn inside_a_run_shell_it_refuses_with_exit_1_and_takes_no_lock() {
    // D4.1 step 1, D8.1.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = staying(d.path());
    let session = Env::for_test(d.path()).data_dir().join("sessions").join(&a);
    fs::create_dir_all(&session).unwrap();
    ProfileMarker {
        provider: ProviderId::new(CLAUDE_CODE),
        account_id: AccountId::from_string(&a),
        config_dir: canonical_profile_path(&session)
            .unwrap()
            .display()
            .to_string(),
        outer: json!({"CLAUDE_CONFIG_DIR": null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": null}),
    }
    .write(&session)
    .unwrap();
    let in_shell = |args: &[&str]| {
        let mut c = daemon(d.path(), args);
        c.env("CLAUDE_CONFIG_DIR", &session);
        Running::spawn(c).finish(Duration::from_secs(20))
    };
    let out = in_shell(&[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(text(&out.stderr), format!("tagteam: {INSIDE}\n"));
    assert_eq!(out.stdout, b"");
    let out = in_shell(&["--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "inside-run-shell", "message": INSIDE}})
    );
    assert!(!lock(d.path(), "daemon.lock").exists());
}

#[test]
fn a_second_daemon_says_it_waits_and_takes_over_when_the_first_stops() {
    // D4.1 step 2: one daemon per data directory. The second says so once on stderr, runs
    // nothing while it waits, and tries again every 5 s.
    let d = tempfile::tempdir().unwrap();
    staying(d.path());
    let mut first = Running::spawn(daemon(d.path(), &["--json"]));
    first.wait_for(Duration::from_secs(20), "its first tick", |out| {
        out.contains(r#""event":"no-switch""#)
    });
    let mut second = Running::spawn(daemon(d.path(), &["--json"]));
    let notice = format!(
        "note: a daemon already runs (pid {}); waiting for it to stop\n",
        first.child.id()
    );
    second.wait_for_stderr(Duration::from_secs(20), "its notice", |err| err == notice);
    assert_eq!(second.stdout(), "", "nothing ran while it waited");
    first.signal(libc::SIGTERM);
    assert_eq!(first.finish(Duration::from_secs(10)).status.code(), Some(0));
    second.wait_for(Duration::from_secs(20), "its first tick", |out| {
        out.contains(r#""event":"no-switch""#)
    });
    let record: Value =
        serde_json::from_slice(&fs::read(lock(d.path(), "daemon.lock")).unwrap()).unwrap();
    assert_eq!(record["pid"], json!(second.child.id()));
    second.signal(libc::SIGTERM);
    let out = second.finish(Duration::from_secs(10));
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stderr),
        notice,
        "the notice once, and nothing else"
    );
}

#[test]
fn an_unknown_flag_is_a_usage_error() {
    // D8.1: exit 2.
    let d = tempfile::tempdir().unwrap();
    cmd(d.path()).args(["daemon", "--once"]).assert().code(2);
}
