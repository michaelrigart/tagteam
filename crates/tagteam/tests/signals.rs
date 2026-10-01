//! §14.1 with real signals sent to the binary: SIGINT while a switch waits for Claude Code's
//! lock (Review Focus 1), and twice while a switch is inside its critical span (Review Focus 2).
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::Path;
use std::process::{Child, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::{live_email, std_cmd, two_fresh_accounts};
use serde_json::{Value, json};
use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::store::Store;
use tagteam_provider::{Env, FileKeychain, Keychain};

/// Sends `signal` to the child, as a terminal's Ctrl-C (SIGINT) or `kill` would.
fn send(child: &Child, signal: i32) {
    // SAFETY: kill(2) reads no memory of ours. `child` is a process this test spawned and has
    // not reaped, so its pid names it and no other process.
    let rc = unsafe { libc::kill(child.id() as libc::pid_t, signal) };
    assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());
}

/// Polls `ready` every 10 ms until it holds. Fails the test if the child exits first, or if
/// `within` passes.
fn wait_until(child: &mut Child, within: Duration, what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + within;
    while !ready() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("tagteam exited ({status}) before {what}");
        }
        assert!(Instant::now() < deadline, "tagteam never got to {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// The child's output once it has exited. Fails the test, and kills the child, if it is still
/// running after `within`.
fn finish(mut child: Child, within: Duration) -> Output {
    let deadline = Instant::now() + within;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("tagteam was still running {within:?} later");
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

fn store(root: &Path) -> Store {
    Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
}

/// The live OAuth entry, as Claude Code reads it.
fn live_entry(root: &Path) -> Option<Vec<u8>> {
    let env = Env::for_test(root);
    FileKeychain::new(root.join("keychain"))
        .find(
            &keychain_service(&env, ItemKind::OAuth),
            &keychain_account(&env),
        )
        .present()
}

#[test]
fn ctrl_c_while_a_switch_waits_for_claude_code_s_lock_exits_130_with_nothing_written() {
    // Review Focus 1. Claude Code is refreshing and holds its legacy lock, so the switch takes
    // and releases its own refresh lock over and over while it waits (§9.1).
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (_a, b) = two_fresh_accounts(root);
    let paths = CcPaths::resolve(&Env::for_test(root));
    fs::create_dir(paths.legacy_lock()).unwrap(); // Claude Code's, fresh: never stale here
    let entry = live_entry(root);
    let config_home = paths.refresh_lock.parent().unwrap().to_path_buf();
    let untouched = fs::metadata(&config_home).unwrap().modified().unwrap();

    let mut child = std_cmd(root)
        .args(["switch", "1", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Taking and releasing the refresh lock changes its directory's mtime: by then the switch
    // is in the wait, holding the mutation and account locks.
    wait_until(
        &mut child,
        Duration::from_secs(20),
        "Claude Code's lock",
        || fs::metadata(&config_home).unwrap().modified().unwrap() != untouched,
    );
    let signalled = Instant::now();
    send(&child, libc::SIGINT);
    let out = finish(child, Duration::from_secs(8));

    assert!(
        signalled.elapsed() < Duration::from_secs(2),
        "stopped at the wait's next attempt, not at its 9 s timeout: {:?}",
        signalled.elapsed()
    );
    assert_eq!(
        out.status.code(),
        Some(130),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "interrupted", "message": "interrupted"}})
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert!(
        paths.legacy_lock().is_dir(),
        "Claude Code's lock is left alone"
    );
    assert!(
        !paths.refresh_lock.exists() && !paths.config_lock.exists(),
        "no lock directory of tagteam's is left behind"
    );
    assert_eq!(live_email(root), "b@x.co");
    assert_eq!(live_entry(root), entry, "the live credential is untouched");
    let provider = ProviderId::new(CLAUDE_CODE);
    let s = store(root);
    assert!(
        s.journal(&provider).unwrap().is_none(),
        "the switch never journaled"
    );
    assert_eq!(
        s.active(&provider).unwrap(),
        Some(AccountId::from_string(&b))
    );
}

#[test]
fn ctrl_c_twice_inside_a_switch_s_critical_span_lets_it_commit_and_reports_it_too_late() {
    // Review Focus 2. The switch is parked right after its journal row (§9.4 step 6), before it
    // writes the live credential. Two SIGINTs land there; it then writes and commits.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (a, _b) = two_fresh_accounts(root);
    let pause = root.join("pause");
    fs::create_dir(&pause).unwrap();

    let mut child = std_cmd(root)
        .args(["switch", "1", "--json"])
        .env("TAGTEAM_TEST_PAUSE_AT", "after-journal")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_until(
        &mut child,
        Duration::from_secs(20),
        "its journal row",
        || pause.join("paused").exists(),
    );
    send(&child, libc::SIGINT);
    thread::sleep(Duration::from_millis(100));
    send(&child, libc::SIGINT);
    thread::sleep(Duration::from_millis(100));
    assert!(
        child.try_wait().unwrap().is_none(),
        "neither Ctrl-C stopped the switch midway"
    );
    fs::write(pause.join("resume"), b"").unwrap();
    let out = finish(child, Duration::from_secs(20));

    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (v["switched"].clone(), v["reason"].clone()),
        (json!(true), json!("switched")),
        "{v}"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "tagteam: interrupted too late to stop: switch had already finished\n"
    );
    assert_eq!(live_email(root), "a@x.co");
    let paths = CcPaths::resolve(&Env::for_test(root));
    assert!(
        !paths.refresh_lock.exists()
            && !paths.legacy_lock().exists()
            && !paths.config_lock.exists()
    );
    let provider = ProviderId::new(CLAUDE_CODE);
    let s = store(root);
    assert!(
        s.journal(&provider).unwrap().is_none(),
        "the switch committed: no journal row remains"
    );
    assert_eq!(
        s.active(&provider).unwrap(),
        Some(AccountId::from_string(&a))
    );
}

#[test]
fn a_signal_ignored_at_startup_stays_ignored() {
    // `nohup tagteam ...` and background jobs start with SIGHUP ignored, and tagteam keeps it
    // that way: the switch waits for Claude Code's lock through the signal and then completes.
    use std::os::unix::process::CommandExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (a, _b) = two_fresh_accounts(root);
    let paths = CcPaths::resolve(&Env::for_test(root));
    fs::create_dir(paths.legacy_lock()).unwrap();
    let config_home = paths.refresh_lock.parent().unwrap().to_path_buf();
    let untouched = fs::metadata(&config_home).unwrap().modified().unwrap();

    let mut cmd = std_cmd(root);
    cmd.args(["switch", "1", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: the closure runs between fork and exec and only calls signal(2), which is
    // async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            Ok(())
        });
    }
    let mut child = cmd.spawn().unwrap();
    wait_until(
        &mut child,
        Duration::from_secs(20),
        "Claude Code's lock",
        || fs::metadata(&config_home).unwrap().modified().unwrap() != untouched,
    );
    send(&child, libc::SIGHUP);
    thread::sleep(Duration::from_millis(500));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the ignored SIGHUP neither killed nor stopped the switch"
    );
    fs::remove_dir(paths.legacy_lock()).unwrap();
    let out = finish(child, Duration::from_secs(20));

    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["switched"], json!(true), "{v}");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "", "no notice either");
    assert_eq!(live_email(root), "a@x.co");
    let provider = ProviderId::new(CLAUDE_CODE);
    assert_eq!(
        store(root).active(&provider).unwrap(),
        Some(AccountId::from_string(&a))
    );
}

#[test]
fn ctrl_c_twice_while_the_switch_s_write_waits_for_claude_code_s_storage_write_lock_lets_it_commit()
{
    // Review Focus 2 with Claude Code holding its storage-write lock (§9.1, Task 11): the
    // switch's step 7 write waits for it inside the critical span, and two SIGINTs land during
    // that wait. The switch writes and commits once CC lets go, and says the signal came too
    // late (§14.1).
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (a, _b) = two_fresh_accounts(root);
    let paths = CcPaths::resolve(&Env::for_test(root));
    let pause = root.join("pause");
    fs::create_dir(&pause).unwrap();

    let mut child = std_cmd(root)
        .args(["switch", "1", "--json"])
        .env("TAGTEAM_TEST_PAUSE_AT", "after-journal")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_until(
        &mut child,
        Duration::from_secs(20),
        "its journal row",
        || pause.join("paused").exists(),
    );
    fs::create_dir(&paths.storage_write_lock).unwrap(); // Claude Code takes it
    fs::write(pause.join("resume"), b"").unwrap();
    thread::sleep(Duration::from_millis(300)); // the step 7 write now waits for CC's lock
    send(&child, libc::SIGINT);
    thread::sleep(Duration::from_millis(100));
    send(&child, libc::SIGINT);
    thread::sleep(Duration::from_millis(300));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the switch finished, or a Ctrl-C stopped it, before Claude Code let go of its lock"
    );
    fs::remove_dir(&paths.storage_write_lock).unwrap(); // Claude Code lets go
    let out = finish(child, Duration::from_secs(20));

    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (v["switched"].clone(), v["reason"].clone()),
        (json!(true), json!("switched")),
        "{v}"
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "tagteam: interrupted too late to stop: switch had already finished\n"
    );
    assert_eq!(live_email(root), "a@x.co");
    let provider = ProviderId::new(CLAUDE_CODE);
    let s = store(root);
    assert!(s.journal(&provider).unwrap().is_none());
    assert_eq!(
        s.active(&provider).unwrap(),
        Some(AccountId::from_string(&a))
    );
    assert!(
        !paths.storage_write_lock.exists(),
        "tagteam released the storage-write lock it took"
    );
}
