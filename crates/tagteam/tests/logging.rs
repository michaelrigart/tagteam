//! §14.2 through the real binary: where the log is and with what modes, what its filter
//! takes, the `statusline` fast path that never opens it, a log that cannot be written, a
//! panic's line, and a collector thread's log line under `--debug`. Needs `--features
//! test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{cmd, expire_vault, login, seed_home, std_cmd, two_fresh_accounts};
use serde_json::{Value, json};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Env, FileKeychain};

const TAGTEAM_LOG: &str = "TAGTEAM_LOG";
const API_BASE: &str = "TAGTEAM_TEST_API_BASE";
/// §9.4 step 4's WARN line for a capture nothing could attribute.
const CAPTURED: &str = "captured an unverified live credential";

/// The fixture's default state directory, `~/.local/state/tagteam` (§5).
fn state_dir(root: &Path) -> PathBuf {
    Env::for_test(root).state_dir()
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// The binary with its log file off: a fixture's setup, so only the command under test logs.
fn quiet(root: &Path) -> assert_cmd::Command {
    let mut c = cmd(root);
    c.env(TAGTEAM_LOG, "off");
    c
}

/// `a@x.co` at position 1 and `b@x.co` at 2, `b` live, and then Claude Code rotated b's
/// credential in place. With every endpoint offline nothing attributes the rotation, so
/// `switch 1` captures it into b's vault with a WARN line (§9.4 step 4), which the file
/// records by default and stderr does not, and the oracle's missing answer is a DEBUG line.
fn rotated_live_login(root: &Path) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    quiet(root).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    quiet(root).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b-rotated");
}

/// The log's text, oldest first across its rotations; empty when there is none.
fn log_text(dir: &Path) -> String {
    ["tagteam.log.2", "tagteam.log.1", "tagteam.log"]
        .iter()
        .map(|n| fs::read_to_string(dir.join(n)).unwrap_or_default())
        .collect()
}

/// Decision 8's line: `<UTC time to the millisecond> <pid> <LEVEL> <target>: …`.
fn assert_line_shape(line: &str, level: &str, target: &str) {
    let parts: Vec<&str> = line.splitn(5, ' ').collect();
    assert_eq!(parts.len(), 5, "{line}");
    let time = parts[0].as_bytes();
    assert!(
        time.len() == 24 && time[10] == b'T' && time[19] == b'.' && time[23] == b'Z',
        "{line}"
    );
    assert!(parts[1].parse::<u32>().is_ok(), "{line}");
    assert_eq!((parts[2], parts[3]), (level, format!("{target}:").as_str()));
}

#[test]
fn a_switch_logs_to_a_private_file_in_the_state_directory() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let dir = state_dir(d.path());
    assert!(!dir.exists(), "the setup logged nothing");
    cmd(d.path())
        .args(["switch", "1", "--json"])
        .assert()
        .success()
        .stderr("");
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("tagteam.log")), 0o600);
    let log = log_text(&dir);
    let line = log
        .lines()
        .find(|l| l.contains(CAPTURED))
        .unwrap_or_else(|| panic!("no capture line in:\n{log}"));
    assert_line_shape(line, "WARN", "tagteam_engine::switch");
    assert!(
        !log.contains("the profile oracle gave no answer"),
        "DEBUG stays out by default:\n{log}"
    );
    let home = d.path().join("home").display().to_string();
    assert!(!log.contains(&home), "paths under HOME are ~/…:\n{log}");
}

#[test]
fn debug_adds_tagteam_s_debug_lines_to_the_file() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    cmd(d.path())
        .args(["switch", "1", "--debug"])
        .assert()
        .success();
    let log = log_text(&state_dir(d.path()));
    let line = log
        .lines()
        .find(|l| l.contains("the profile oracle gave no answer"))
        .unwrap_or_else(|| panic!("no DEBUG line in:\n{log}"));
    assert_line_shape(line, "DEBUG", "tagteam_engine::oracle");
}

#[test]
fn xdg_state_home_moves_the_log() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let state = d.path().join("state");
    cmd(d.path())
        .env("XDG_STATE_HOME", &state)
        .args(["switch", "1"])
        .assert()
        .success();
    assert!(log_text(&state.join("tagteam")).contains(CAPTURED));
    assert!(!state_dir(d.path()).exists());
}

#[test]
fn tagteam_log_off_writes_no_file() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    quiet(d.path()).args(["switch", "1"]).assert().success();
    assert!(!state_dir(d.path()).exists());
}

#[test]
fn tagteam_log_replaces_the_whole_file_filter() {
    // Only the switch module, at WARN: nothing else the switch logs reaches the file.
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    cmd(d.path())
        .env(TAGTEAM_LOG, "tagteam_engine::switch=warn")
        .args(["switch", "1"])
        .assert()
        .success();
    let log = log_text(&state_dir(d.path()));
    assert!(log.contains(CAPTURED), "{log}");
    for line in log.lines() {
        assert_eq!(
            line.split(' ').nth(3),
            Some("tagteam_engine::switch:"),
            "{line}"
        );
    }
}

#[test]
fn an_invalid_tagteam_log_warns_once_and_keeps_the_default() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let out = cmd(d.path())
        .env(TAGTEAM_LOG, "tagteam=loud")
        .args(["switch", "1", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.starts_with("warning: TAGTEAM_LOG is not a valid filter")
            && stderr.lines().count() == 1,
        "{stderr}"
    );
    // B.36: the warning is on stderr; stdout is still exactly one object.
    serde_json::from_slice::<Value>(&out.stdout).unwrap();
    assert!(log_text(&state_dir(d.path())).contains(CAPTURED));
}

#[test]
fn statusline_with_the_default_filter_opens_no_log() {
    // §14.2, Review Focus 4: the status bar runs every few seconds and logs at DEBUG at most,
    // so by default it never creates, opens or rotates the log.
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let out = cmd(d.path())
        .arg("statusline")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(!out.is_empty());
    assert!(!state_dir(d.path()).exists());
}

#[test]
fn a_log_that_cannot_be_written_fails_no_command() {
    // §14.2, §15.2. The state directory is under a regular file, so it can never be made, by
    // root either. Silent by default; `--debug` says so, once.
    for debug in [false, true] {
        let d = tempfile::tempdir().unwrap();
        rotated_live_login(d.path());
        fs::write(d.path().join("blocker"), "").unwrap();
        let mut c = cmd(d.path());
        c.env("XDG_STATE_HOME", d.path().join("blocker/state"))
            .args(["switch", "1", "--json"]);
        if debug {
            c.arg("--debug");
        }
        let out = c.assert().success().get_output().clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap()["switched"],
            true
        );
        let stderr = String::from_utf8(out.stderr).unwrap();
        let notices = stderr.matches("cannot be written").count();
        assert_eq!(notices, usize::from(debug), "{stderr}");
    }
}

#[test]
fn a_panic_is_logged_once_with_its_location_and_stderr_keeps_the_default_hook() {
    // Decision 9: logged at ERROR before the process unwinds, and left out of stderr, where
    // the default hook prints the panic.
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let out = cmd(d.path())
        .env("TAGTEAM_TEST_FAIL_AT", "panic:after-journal")
        .args(["switch", "1"])
        .assert()
        .code(101)
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("panicked at") && stderr.contains("injected panic at after-journal"),
        "{stderr}"
    );
    assert!(!stderr.contains("location="), "{stderr}");
    let log = log_text(&state_dir(d.path()));
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| l.contains(" tagteam::panic: "))
        .collect();
    assert_eq!(lines.len(), 1, "{log}");
    assert_line_shape(lines[0], "ERROR", "tagteam::panic");
    assert!(
        lines[0].contains("location=crates/tagteam-engine/src/hooks.rs:"),
        "{}",
        lines[0]
    );
    assert!(
        !lines[0].contains("after-journal"),
        "a formatted message is left to stderr: {}",
        lines[0]
    );
}

#[test]
fn a_collector_thread_s_log_line_under_debug_never_hangs_the_command() {
    // `list` collects each account on a thread of its own (§8.3). a's refresh is refused with
    // invalid_grant, so the gate quarantines a from that thread with a WARN line, which
    // `--debug` prints on stderr. The main thread waits for that thread meanwhile: it must not
    // be holding stderr.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let (a, _b) = two_fresh_accounts(root);
    expire_vault(root, &a, -60_000);
    let server = MockServer::start();
    server.on(
        "POST",
        "/v1/oauth/token",
        MockReply::Json {
            status: 400,
            body: json!({"error": "invalid_grant"}),
        },
    );
    let mut child = std_cmd(root)
        .env(API_BASE, server.base_url())
        .args(["list", "--debug"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("`list --debug` hung");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("quarantined"), "{stderr}");
    assert_eq!(server.hits("POST", "/v1/oauth/token"), 1);
}
