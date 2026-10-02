//! `tagteam run` through the real binary (Task 12 on), and the fake `claude` those tests put
//! first on `PATH`. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use common::{FakeCall, fake_claude, fake_claude_calls, path_with};
use serde_json::{Value, json};

/// The fake `claude` run on its own, as tagteam would find it: first on `PATH`, `HOME` under
/// `root`, in `root`, and nothing else in its environment.
fn fake(root: &Path, args: &[&str]) -> Command {
    let bin = fake_claude(root);
    let mut c = Command::new(bin.join("claude"));
    c.args(args)
        .env_clear()
        .env("HOME", root.join("home"))
        .env("PATH", path_with(&bin))
        .current_dir(root);
    c
}

/// Waits up to 10 s for `done` to hold of what the fake recorded in `out`.
fn wait_for(out: &Path, what: &str, done: impl Fn(&[FakeCall]) -> bool) -> Vec<FakeCall> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let calls = fake_claude_calls(out);
        if done(&calls) {
            return calls;
        }
        assert!(Instant::now() < deadline, "no {what}: {calls:?}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// Sends `signal` to `pid` alone, as `kill` does.
fn send(pid: u32, signal: libc::c_int) {
    // SAFETY: kill(2) reads no memory of ours. `pid` is a process this test started and has
    // not reaped, or that process's child, whose pid the fake recorded while it runs.
    let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
    assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn the_fake_claude_records_its_arguments_environment_and_pid() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("calls");
    let mut child = fake(d.path(), &["a b", "", "--json"])
        .arg(OsStr::from_bytes(b"caf\xe9"))
        .env("FAKE_CLAUDE_OUT", &out)
        .env("MARK", "on")
        .spawn()
        .unwrap();
    let pid = child.id();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    let calls = fake_claude_calls(&out);
    assert_eq!(calls.len(), 1, "{calls:?}");
    let call = &calls[0];
    assert_eq!(
        (call.pid, call.ppid, call.mode.as_str()),
        (pid, std::process::id(), "session")
    );
    assert_eq!(
        call.args,
        [
            OsString::from("a b"),
            OsString::from(""),
            OsString::from("--json"),
            OsString::from_vec(b"caf\xe9".to_vec()),
        ],
        "every argument as its bytes, one that is not UTF-8 included"
    );
    assert_eq!(call.cwd, fs::canonicalize(d.path()).unwrap());
    assert_eq!(call.env.get("MARK").map(String::as_str), Some("on"));
    assert_eq!(
        call.env.get("HOME").map(PathBuf::from),
        Some(d.path().join("home"))
    );
    assert!(call.ready);
    assert!(call.signals.is_empty());
    assert_eq!(call.exit, Some(0));
}

#[test]
fn the_fake_claude_exits_with_the_code_it_is_given() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("calls");
    let status = fake(d.path(), &[])
        .env("FAKE_CLAUDE_EXIT", "7")
        .env("FAKE_CLAUDE_OUT", &out)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(7));
    assert_eq!(fake_claude_calls(&out)[0].exit, Some(7));
}

#[test]
fn the_fake_claude_auth_status_is_logged_out_unless_a_login_is_given() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("calls");
    let reply = |vars: &[(&str, &str)]| {
        let mut c = fake(d.path(), &["auth", "status", "--json"]);
        c.env("FAKE_CLAUDE_OUT", &out);
        for (k, v) in vars {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        (
            o.status.code(),
            serde_json::from_slice::<Value>(&o.stdout).unwrap(),
        )
    };
    // Claude Code exits non-zero when logged out (Appendix A.7), and still names its config
    // home. Nothing here holds a credential, so no login is read from a profile either.
    let default_home = d.path().join("home/.claude");
    assert_eq!(
        reply(&[]),
        (
            Some(1),
            json!({"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty",
                   "configDirectory": default_home.to_str().unwrap()})
        )
    );
    let profile = d.path().join("profile");
    let profile = profile.to_str().unwrap();
    assert_eq!(
        reply(&[
            ("CLAUDE_CONFIG_DIR", profile),
            ("FAKE_CLAUDE_EMAIL", "a@x.co"),
            ("FAKE_CLAUDE_ORG", "org-1"),
        ]),
        (
            Some(0),
            json!({"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty",
                   "configDirectory": profile, "email": "a@x.co", "orgId": "org-1",
                   "subscriptionType": "max"})
        )
    );
    let (_, plain) = reply(&[("FAKE_CLAUDE_EMAIL", "a@x.co")]);
    assert_eq!(
        plain["configDirectory"],
        d.path().join("home/.claude").to_str().unwrap(),
        "without CLAUDE_CONFIG_DIR, the default home"
    );
    assert!(plain.get("orgId").is_none(), "{plain}");
    assert_eq!(
        reply(&[
            ("FAKE_CLAUDE_AUTH", "{\"authMethod\": \"api_key\"}"),
            ("FAKE_CLAUDE_AUTH_EXIT", "3"),
        ]),
        (Some(3), json!({"authMethod": "api_key"}))
    );
    let calls = fake_claude_calls(&out);
    assert_eq!(calls.len(), 4);
    assert!(
        calls
            .iter()
            .all(|c| c.mode == "auth" && c.args == ["auth", "status", "--json"] && !c.ready),
        "{calls:?}"
    );
}

#[test]
fn the_fake_claude_auth_status_takes_as_long_as_it_is_told() {
    let d = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let out = fake(d.path(), &["auth", "status"])
        .env("FAKE_CLAUDE_AUTH_SLEEP", "0.3")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(started.elapsed() >= Duration::from_millis(300));
}

#[test]
fn a_graceful_exit_removes_the_fake_s_session_record_and_a_kill_leaves_it() {
    // Claude Code removes its record on SIGINT, SIGTERM and SIGHUP, and SIGKILL leaves it
    // behind (§12.6).
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("calls");
    let profile = d.path().join("profile");
    let start = || {
        fake(d.path(), &[])
            .env("CLAUDE_CONFIG_DIR", &profile)
            .env("FAKE_CLAUDE_OUT", &out)
            .env("FAKE_CLAUDE_RECORD", "interactive")
            .env("FAKE_CLAUDE_SLEEP", "30")
            .spawn()
            .unwrap()
    };
    let mut child = start();
    let pid = child.id();
    wait_for(&out, "running session", |c| {
        c.iter().any(|c| c.pid == pid && c.ready)
    });
    let record = profile.join(format!("sessions/{pid}.json"));
    let v: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(v["pid"].as_u64(), Some(u64::from(pid)), "{v}");
    assert_eq!(v["kind"], "interactive");
    assert!(
        v["startedAt"]
            .as_i64()
            .is_some_and(|ms| ms > 1_700_000_000_000)
    );
    assert_eq!(mode(&profile.join("sessions")), 0o700);
    send(pid, libc::SIGTERM);
    assert_eq!(child.wait().unwrap().code(), Some(143));
    assert!(!record.exists(), "a graceful exit removes it");
    let call = fake_claude_calls(&out)
        .into_iter()
        .find(|c| c.pid == pid)
        .unwrap();
    assert_eq!((call.signals, call.exit), (vec![15], Some(143)));

    let mut child = start();
    let pid = child.id();
    wait_for(&out, "running session", |c| {
        c.iter().any(|c| c.pid == pid && c.ready)
    });
    send(pid, libc::SIGKILL);
    assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
    assert!(
        profile.join(format!("sessions/{pid}.json")).exists(),
        "SIGKILL leaves it behind"
    );
}

#[test]
fn with_continue_the_fake_claude_records_each_signal_and_runs_on() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("calls");
    let mut child = fake(d.path(), &[])
        .env("FAKE_CLAUDE_OUT", &out)
        .env("FAKE_CLAUDE_SLEEP", "30")
        .env("FAKE_CLAUDE_ON_SIGNAL", "continue")
        .spawn()
        .unwrap();
    wait_for(&out, "running session", |c| {
        c.first().is_some_and(|c| c.ready)
    });
    send(child.id(), libc::SIGINT);
    wait_for(&out, "SIGINT", |c| c[0].signals == [2]);
    send(child.id(), libc::SIGTERM);
    wait_for(&out, "SIGTERM", |c| c[0].signals == [2, 15]);
    assert!(child.try_wait().unwrap().is_none(), "still running");
    send(child.id(), libc::SIGKILL);
    child.wait().unwrap();
}

#[test]
fn with_a_hold_file_the_fake_claude_runs_until_it_exists() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("calls");
    let hold = d.path().join("hold");
    let mut child = fake(d.path(), &[])
        .env("FAKE_CLAUDE_OUT", &out)
        .env("FAKE_CLAUDE_HOLD", &hold)
        .spawn()
        .unwrap();
    wait_for(&out, "running session", |c| {
        c.first().is_some_and(|c| c.ready)
    });
    thread::sleep(Duration::from_millis(300));
    assert!(child.try_wait().unwrap().is_none(), "held");
    fs::write(&hold, b"").unwrap();
    let started = Instant::now();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "it let go at once"
    );
    assert_eq!(fake_claude_calls(&out)[0].exit, Some(0));
}

#[test]
fn the_fake_claude_auth_status_follows_its_config_home() {
    // As Claude Code's does (Appendix A.7): a credential and an `oauthAccount` make a claude.ai
    // login, and an `apiKeyHelper` in the settings wins over it.
    let d = tempfile::tempdir().unwrap();
    let profile = d.path().join("profile");
    fs::create_dir_all(&profile).unwrap();
    let spelling = profile.to_str().unwrap();
    let reply = || {
        let o = fake(d.path(), &["auth", "status", "--json"])
            .env("CLAUDE_CONFIG_DIR", &profile)
            .output()
            .unwrap();
        (
            o.status.code(),
            serde_json::from_slice::<Value>(&o.stdout).unwrap(),
        )
    };
    let config = json!({"oauthAccount": {"emailAddress": "a@x.co", "organizationUuid": "org-1"}});
    fs::write(
        profile.join(".claude.json"),
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
    assert_eq!(reply().0, Some(1), "no credential: logged out");
    fs::write(profile.join(".credentials.json"), "{}").unwrap();
    assert_eq!(
        reply(),
        (
            Some(0),
            json!({"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty",
                   "configDirectory": spelling, "email": "a@x.co", "orgId": "org-1",
                   "subscriptionType": "max"})
        )
    );
    fs::write(
        profile.join("settings.json"),
        r#"{"apiKeyHelper": "~/bin/key"}"#,
    )
    .unwrap();
    assert_eq!(
        reply(),
        (
            Some(0),
            json!({"loggedIn": true, "authMethod": "api_key_helper", "apiProvider": "firstParty",
                   "apiKeySource": "apiKeyHelper", "configDirectory": spelling})
        )
    );
}

#[test]
fn the_fake_claude_rotates_its_profile_credential_privately() {
    let d = tempfile::tempdir().unwrap();
    let profile = d.path().join("profile");
    fs::create_dir_all(&profile).unwrap();
    let rotated = json!({"claudeAiOauth": {"refreshToken": "rt-next"}}).to_string();
    let status = fake(d.path(), &[])
        .env("CLAUDE_CONFIG_DIR", &profile)
        .env("FAKE_CLAUDE_ROTATE", &rotated)
        .status()
        .unwrap();
    assert!(status.success());
    let file = profile.join(".credentials.json");
    assert_eq!(fs::read_to_string(&file).unwrap(), rotated);
    assert_eq!(mode(&file), 0o600);
}
