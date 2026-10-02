//! `tagteam auto` through the real binary (§11.1, §11.4, §14.1). Readings are recorded through
//! the store, fresh and with a plan in force, so a tick's scheduled collection sends nothing;
//! every endpoint stays offline (`std_cmd`'s default). Every run is drained while it runs
//! (`Running`), since a loop prints until it is stopped. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

use common::{
    Running, expire_vault, live_email, now_epoch_s, record_reading, std_cmd, two_fresh_accounts,
    usage_window,
};
use serde_json::{Value, json};
use tagteam_cc::CcPaths;
use tagteam_core::autoswitch::AutoState;
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId, Window, WindowKind};
use tagteam_engine::store::Store;
use tagteam_provider::Env;
use tagteam_provider::profile::{ProfileMarker, canonical_profile_path};

/// A reading taken at `now`: 5h at `five` and 7d at `seven`, resetting in 2h40m30s and
/// 3d09h00m30s.
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

/// `a@x.co` at position 1 read at (5h, 7d) `a`, and `b@x.co` at position 2, live, read at `b`.
fn accounts(root: &Path, a: (f64, f64), b: (f64, f64)) -> (String, String) {
    let (id_a, id_b) = two_fresh_accounts(root);
    let now = now_epoch_s();
    record_reading(root, &id_a, now, &reading(now, a.0, a.1));
    record_reading(root, &id_b, now, &reading(now, b.0, b.1));
    (id_a, id_b)
}

/// b at 95 % of its 7d window, a at 20 %: a tick switches to a (proactive).
fn switching(root: &Path) -> (String, String) {
    accounts(root, (10.0, 20.0), (10.0, 95.0))
}

/// b at 60 %: below the threshold, a tick stays.
fn staying(root: &Path) -> (String, String) {
    accounts(root, (10.0, 20.0), (10.0, 60.0))
}

fn auto_cmd(root: &Path, args: &[&str]) -> std::process::Command {
    let mut cmd = std_cmd(root);
    cmd.arg("auto").args(args);
    cmd
}

/// `tagteam auto <args>`, run to its end.
fn auto(root: &Path, args: &[&str]) -> Output {
    Running::spawn(auto_cmd(root, args)).finish(Duration::from_secs(20))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
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

/// Each stdout line as JSON, its `ts` checked as ISO 8601 UTC and then replaced by `[ts]`.
fn events(bytes: &[u8]) -> Vec<Value> {
    text(bytes)
        .lines()
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).unwrap();
            let ts = v["ts"].as_str().unwrap();
            assert!(
                ts.len() == 20 && ts.ends_with('Z') && ts.as_bytes()[10] == b'T',
                "{ts}"
            );
            v["ts"] = json!("[ts]");
            v
        })
        .collect()
}

fn store(root: &Path) -> Store {
    Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
}

fn cc() -> ProviderId {
    ProviderId::new(CLAUDE_CODE)
}

#[test]
fn once_exits_0_switched_1_error_2_no_action_and_3_blocked() {
    // §11.4. One line per tick on stdout, errors on stderr.
    let d = tempfile::tempdir().unwrap();
    staying(d.path());
    let out = auto(d.path(), &["--once"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert_eq!(
        untimed(&out.stdout),
        ["#2 b@x.co  60%  no switch: below-threshold"]
    );
    assert_eq!(text(&out.stderr), "");

    let d = tempfile::tempdir().unwrap();
    switching(d.path());
    let out = auto(d.path(), &["--once"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        untimed(&out.stdout),
        ["#2 b@x.co  95%  switched to #1 (proactive)"]
    );
    assert_eq!(live_email(d.path()), "a@x.co");

    // Every candidate at its limit: blocked until the first one is back.
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), (100.0, 50.0), (10.0, 95.0));
    let out = auto(d.path(), &["--once"]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
    let line = &untimed(&out.stdout)[0];
    assert!(
        line.starts_with("#2 b@x.co  95%  no switch: all-exhausted, the first back at ")
            && line.ends_with(" (2h40m)"),
        "{line}"
    );

    // a's access token needs a refresh that cannot be sent: §11.2 step 12's error.
    let d = tempfile::tempdir().unwrap();
    let (a, _b) = switching(d.path());
    expire_vault(d.path(), &a, 60_000);
    let out = auto(d.path(), &["--once"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert_eq!(out.stdout, b"");
    let err = untimed(&out.stderr);
    assert!(
        err.len() == 1 && err[0].starts_with("error: could not freshen a@x.co (position 1): "),
        "{err:?}"
    );
    assert_eq!(live_email(d.path()), "b@x.co");
}

#[test]
fn json_prints_one_event_per_line_in_section_11_4_s_shape() {
    let d = tempfile::tempdir().unwrap();
    switching(d.path());
    let out = auto(d.path(), &["--once", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).starts_with(r#"{"schemaVersion":1,"event":"poll","ts":""#),
        "the envelope comes first"
    );
    assert_eq!(
        events(&out.stdout),
        [
            json!({"schemaVersion": 1, "event": "poll", "ts": "[ts]", "provider": "claude-code",
                   "active": {"number": 2, "email": "b@x.co"},
                   "headroomPct": {"1": 80.0, "2": 5.0}, "threshold": 90.0,
                   "windowsPct": {"1": {"5h": 10.0, "7d": 20.0}, "2": {"5h": 10.0, "7d": 95.0}}}),
            json!({"schemaVersion": 1, "event": "switch", "ts": "[ts]", "provider": "claude-code",
                   "trigger": "proactive", "from": 2, "to": 1, "warnings": [], "dryRun": false}),
        ]
    );
    assert_eq!(text(&out.stderr), "");
}

#[test]
fn dry_run_says_what_it_would_switch_and_writes_nothing() {
    // §11.1: no engine lock, no auto-switch state, no switch.
    let d = tempfile::tempdir().unwrap();
    switching(d.path());
    let before = store(d.path()).events().unwrap().len();
    let out = auto(d.path(), &["--once", "--dry-run", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        events(&out.stdout)[1],
        json!({"schemaVersion": 1, "event": "switch", "ts": "[ts]", "provider": "claude-code",
               "trigger": "proactive", "from": 2, "to": 1, "warnings": [], "dryRun": true})
    );
    assert_eq!(live_email(d.path()), "b@x.co");
    let s = store(d.path());
    assert_eq!(s.events().unwrap().len(), before);
    assert_eq!(s.autoswitch_state(&cc()).unwrap(), AutoState::default());
    let lock = Env::for_test(d.path())
        .data_dir()
        .join("locks/autoswitch-claude-code.lock");
    assert!(!lock.exists());
    let out = auto(d.path(), &["--once", "--dry-run"]);
    assert_eq!(
        untimed(&out.stdout),
        ["#2 b@x.co  95%  would switch to #1 (proactive)"]
    );
}

#[test]
fn inside_a_run_shell_auto_refuses_unless_it_is_a_dry_run() {
    // §11.1, like every command that changes the live login (§9.2).
    let d = tempfile::tempdir().unwrap();
    let (a, _) = switching(d.path());
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
        let mut cmd = auto_cmd(d.path(), args);
        cmd.env("CLAUDE_CONFIG_DIR", &session);
        Running::spawn(cmd).finish(Duration::from_secs(20))
    };
    let out = in_shell(&["--once"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stderr),
        "tagteam: this command cannot run inside a `tagteam run` session\n"
    );
    let out = in_shell(&["--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "inside-run-shell",
               "message": "this command cannot run inside a `tagteam run` session"}})
    );
    // The refusal comes first, whatever else would stop `auto`.
    common::cmd(d.path())
        .args(["disable", "1"])
        .assert()
        .success();
    let out = in_shell(&["--once"]);
    assert_eq!(
        (out.status.code(), text(&out.stderr)),
        (
            Some(1),
            "tagteam: this command cannot run inside a `tagteam run` session\n".into()
        )
    );
    common::cmd(d.path())
        .args(["enable", "1"])
        .assert()
        .success();
    // A dry run is not refused: it runs a tick. What it reads there (the default home's login
    // or the session's own) is §12.8's (M4) to settle, so only that it ran is pinned.
    let out = in_shell(&["--once", "--dry-run", "--json"]);
    assert_ne!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert_ne!(
        serde_json::from_slice::<Value>(&out.stdout).ok(),
        Some(
            json!({"schemaVersion": 1, "error": {"type": "inside-run-shell",
               "message": "this command cannot run inside a `tagteam run` session"}})
        )
    );
    let outcomes: Vec<Value> = events(&out.stdout)
        .into_iter()
        .filter(|e| ["no-switch", "switch"].contains(&e["event"].as_str().unwrap()))
        .collect();
    assert_eq!(outcomes.len(), 1, "{}", text(&out.stdout));
    if outcomes[0]["event"] == "switch" {
        assert_eq!(outcomes[0]["dryRun"], json!(true));
    }
    assert_eq!(live_email(d.path()), "b@x.co");
}

#[test]
fn the_loop_stops_cleanly_on_sigterm_and_sigint_after_a_tick() {
    // §11.4: the loop exits 0, with no notice that the signal came too late, and every lock
    // its switch took is released: no Claude Code lock directory is left behind.
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let d = tempfile::tempdir().unwrap();
        switching(d.path());
        let mut running = Running::spawn(auto_cmd(d.path(), &["--json"]));
        running.wait_for(Duration::from_secs(20), "the first switch", |out| {
            out.contains(r#""event":"switch""#)
        });
        running.signal(signal);
        let out = running.finish(Duration::from_secs(5));
        assert_eq!(
            out.status.code(),
            Some(0),
            "{signal}: {}",
            text(&out.stderr)
        );
        assert_eq!(text(&out.stderr), "", "{signal}");
        assert_eq!(live_email(d.path()), "a@x.co");
        let paths = CcPaths::resolve(&Env::for_test(d.path()));
        for lock in [
            paths.refresh_lock.clone(),
            paths.legacy_lock(),
            paths.config_lock.clone(),
            paths.storage_write_lock.clone(),
        ] {
            assert!(!lock.exists(), "{} was left behind", lock.display());
        }
    }
}

#[test]
fn the_loop_stops_by_itself_once_its_output_has_no_reader() {
    // `tagteam auto --json | head -n 1`: Rust ignores SIGPIPE, so the write fails instead, and
    // that stops the command as SIGPIPE would, at the loop's next cancellation point: exit 0, no
    // lock left behind (so a restarted consumer is not told `engine-running`). The shortest
    // interval, so the tick after the reader left comes soon.
    let d = tempfile::tempdir().unwrap();
    staying(d.path());
    let running =
        Running::spawn_closing_stdout_after(auto_cmd(d.path(), &["--json", "--interval", "15"]), 1);
    let out = running.finish(Duration::from_secs(40));
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).starts_with(r#"{"schemaVersion":1,"event":"poll""#),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(text(&out.stderr), "");
    let paths = CcPaths::resolve(&Env::for_test(d.path()));
    for lock in [
        paths.refresh_lock.clone(),
        paths.legacy_lock(),
        paths.config_lock.clone(),
        paths.storage_write_lock.clone(),
    ] {
        assert!(!lock.exists(), "{} was left behind", lock.display());
    }
    // The engine lock is free: a restarted consumer's tick runs.
    let out = auto(d.path(), &["--once", "--json"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains(r#""reason":"below-threshold""#),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn a_second_loop_refuses_and_once_reports_engine_running_while_one_runs() {
    // Review Focus 3, the CLI's half: one engine per provider per machine (§11.1). Nothing is
    // ticked twice: the refused runs never poll.
    let d = tempfile::tempdir().unwrap();
    staying(d.path());
    let mut first = Running::spawn(auto_cmd(d.path(), &["--json"]));
    first.wait_for(Duration::from_secs(20), "its first tick", |out| {
        out.contains(r#""event":"no-switch""#)
    });

    let out = auto(d.path(), &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stderr),
        "tagteam: auto-switch already runs for Claude Code\n"
    );
    assert_eq!(out.stdout, b"");
    let out = auto(d.path(), &["--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "engine-running",
               "message": "auto-switch already runs for Claude Code"}})
    );

    let out = auto(d.path(), &["--once", "--json"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert_eq!(
        events(&out.stdout),
        [
            json!({"schemaVersion": 1, "event": "no-switch", "ts": "[ts]", "provider": "claude-code",
                   "reason": "engine-running", "detail": ""})
        ]
    );
    let out = auto(d.path(), &["--once"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(out.stdout, b"");
    assert_eq!(
        untimed(&out.stderr),
        ["warning: auto-switch already runs for Claude Code in another process; skipping it"]
    );

    first.signal(libc::SIGTERM);
    let out = first.finish(Duration::from_secs(5));
    assert_eq!(out.status.code(), Some(0));
    let polls = events(&out.stdout)
        .iter()
        .filter(|e| e["event"] == "poll")
        .count();
    assert_eq!(polls, 1);
}

#[test]
fn bad_flag_values_are_usage_errors() {
    let d = tempfile::tempdir().unwrap();
    staying(d.path());
    for (args, message) in [
        (
            &["--threshold", "nan"][..],
            "--threshold takes a number from 50 to 99.9",
        ),
        (
            &["--include-api-key-accounts", "maybe"][..],
            "--include-api-key-accounts takes true, false, 1, 0, yes or no",
        ),
    ] {
        let mut all = vec!["--once", "--json"];
        all.extend_from_slice(args);
        let out = auto(d.path(), &all);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap(),
            json!({"schemaVersion": 1, "error": {"type": "usage", "message": message}})
        );
    }
    // In range after clamping, and spelled as §6.4 spells booleans: accepted.
    let out = auto(
        d.path(),
        &[
            "--once",
            "--threshold",
            "120",
            "--interval",
            "1",
            "--include-api-key-accounts",
            "yes",
        ],
    );
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert_eq!(
        untimed(&out.stdout),
        ["#2 b@x.co  60%  no switch: below-threshold"]
    );
}
