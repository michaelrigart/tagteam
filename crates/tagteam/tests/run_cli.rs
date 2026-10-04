//! `tagteam run` through the real binary (Task 12 on), and the fake `claude` those tests put
//! first on `PATH`. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::{
    FakeCall, cc_profile, cmd, expire_vault, fake_claude, fake_claude_calls, live_email, path_with,
    seed_home, std_cmd, two_fresh_accounts,
};
use serde_json::{Value, json};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::AccountId;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::flock::{LockProbe, probe_lock};
use tagteam_provider::mock_server::MockServer;
use tagteam_provider::splice::{get_top_level, replace_top_level};
use tagteam_provider::{
    Env, FileKeychain, Keychain, MutationGuard, ProfileMarker, Read, profile_path,
};

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
            .env("FAKE_CLAUDE_HOLD", d.path().join("never"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
        .env("FAKE_CLAUDE_HOLD", d.path().join("never"))
        .env("FAKE_CLAUDE_ON_SIGNAL", "continue")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
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

// ---- Task 12: `tagteam run` through the binary (§12.1, §12.5, §14.1) ----

/// How long one step of these tests may take before it counts as stuck.
const LONG: Duration = Duration::from_secs(20);
/// Six of the wait loop's 50 ms looks: long enough for anything tagteam would forward to land.
const SETTLE: Duration = Duration::from_millis(300);
/// §12.4's baseline: present while a merge-back is owed.
const BASELINE: &str = ".tagteam-baseline.json";

/// `a@x.co` at position 1 and `b@x.co` at position 2, the live login (`two_fresh_accounts`),
/// with the fake `claude` in `<root>/bin` and a working directory `<root>/work`. `run 1` therefore
/// launches a session; `run 2` would run plain `claude` (§12.1).
struct Home {
    dir: tempfile::TempDir,
    /// Account `a`'s id.
    a: String,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (a, _b) = two_fresh_accounts(dir.path());
        fake_claude(dir.path());
        fs::create_dir_all(dir.path().join("work")).unwrap();
        Home { dir, a }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// A fresh `FAKE_CLAUDE_OUT` file for one step's runs of the fake `claude`.
    fn out(&self, name: &str) -> PathBuf {
        let dir = self.root().join("calls");
        fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// The binary in `<root>/work`, with the fake `claude` first on `PATH`, recording its runs
    /// in `out`.
    fn tagteam(&self, out: &Path) -> Command {
        let mut c = std_cmd(self.root());
        c.env("PATH", path_with(&self.root().join("bin")))
            .env("FAKE_CLAUDE_OUT", out)
            .current_dir(self.root().join("work"));
        c
    }

    /// Account `a`'s profile (§12.2).
    fn profile(&self) -> PathBuf {
        profile_path(
            &Env::for_test(self.root()),
            &AccountId::from_string(&self.a),
        )
    }
}

/// The session's run of the fake `claude` recorded in `out`, if it started.
fn session(out: &Path) -> Option<FakeCall> {
    fake_claude_calls(out)
        .into_iter()
        .find(|c| c.mode == "session")
}

/// The session recorded in `out` is running: its record and rotation are written.
fn started(out: &Path) -> bool {
    session(out).is_some_and(|c| c.ready)
}

/// The login checks (`claude auth status`) recorded in `out`.
fn login_checks(out: &Path) -> Vec<FakeCall> {
    fake_claude_calls(out)
        .into_iter()
        .filter(|c| c.mode == "auth")
        .collect()
}

/// The signals the session recorded in `out` caught, in order.
fn caught(out: &Path) -> Vec<i32> {
    session(out).map(|c| c.signals).unwrap_or_default()
}

/// Starts `c` as a shell starts a foreground job: leading a process group of its own, so a
/// terminal's Ctrl-C or Ctrl-\ can be sent to everything in it (`send_group`).
fn spawn(mut c: Command) -> Child {
    c.process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Sends `signal` to every process in the group `pgid` leads, as a terminal sends Ctrl-C or
/// Ctrl-\ to its foreground group.
fn send_group(pgid: u32, signal: i32) {
    // SAFETY: kill(2) reads no memory of ours. A negative pid names the process group `spawn`
    // made this child lead.
    let rc = unsafe { libc::kill(-(pgid as libc::pid_t), signal) };
    assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());
}

/// Polls `ready` every 10 ms until it holds. Fails if `child` exits first, or after `LONG`.
fn wait_while_running(child: &mut Child, what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + LONG;
    while !ready() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("tagteam exited ({status}) before {what}");
        }
        assert!(Instant::now() < deadline, "tagteam never got to {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// The child's output once it has exited. Kills it and fails if it still runs after `within`.
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

fn settle() {
    thread::sleep(SETTLE);
}

/// The reservation files in `profile` (§12.5), whoever holds them.
fn reservations(profile: &Path) -> Vec<PathBuf> {
    fs::read_dir(profile.join(".tagteam-launch"))
        .map(|d| {
            d.map(|e| e.unwrap().path())
                .filter(|p| p.extension().is_some_and(|x| x == "lock"))
                .collect()
        })
        .unwrap_or_default()
}

/// The spelling `profile`'s marker records: its `CLAUDE_CONFIG_DIR` (§12.2).
fn marker_spelling(profile: &Path) -> String {
    let Read::Present(marker) = ProfileMarker::read(profile) else {
        panic!("{} has no readable marker", profile.display());
    };
    marker.config_dir
}

/// The `error.type` of the one JSON object on `stdout`.
fn kind(stdout: &[u8]) -> String {
    let v: Value = serde_json::from_slice(stdout).unwrap();
    v["error"]["type"].as_str().unwrap().to_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn release(hold: &Path) {
    fs::write(hold, b"").unwrap();
}

/// Creates each hold file when dropped, so a failing test never leaves a fake `claude` running
/// out its 30 s.
struct Release(Vec<PathBuf>);

impl Drop for Release {
    fn drop(&mut self) {
        for hold in &self.0 {
            let _ = fs::write(hold, b"");
        }
    }
}

/// `tagteam run 1 -- session`: account `a`'s session, holding until `hold` exists.
fn start(home: &Home, out: &Path, hold: &Path, extra: &[(&str, &str)]) -> Child {
    let mut c = home.tagteam(out);
    c.args(["run", "1", "--", "session"])
        .env("FAKE_CLAUDE_HOLD", hold);
    for (k, v) in extra {
        c.env(k, v);
    }
    spawn(c)
}

// ---- The plain path: `exec` (§12.1) ----

#[test]
fn plain_claude_is_exec_ed_in_place_with_the_environment_and_arguments_it_was_given() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    fs::create_dir_all(root.join("home")).unwrap();
    let bin = fake_claude(root);
    let out = root.join("calls");
    let mut c = std_cmd(root);
    c.env("PATH", path_with(&bin))
        .env("FAKE_CLAUDE_OUT", &out)
        .env("FAKE_CLAUDE_EXIT", "5")
        .env("ANTHROPIC_API_KEY", "sk-ant-plain")
        .args(["run", "--"])
        .arg("--json")
        .arg("two words")
        .arg(OsStr::from_bytes(b"not \xffutf-8"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = c.spawn().unwrap();
    let pid = child.id();
    let output = finish(child, LONG);

    assert_eq!(output.status.code(), Some(5), "{}", stderr(&output));
    let calls = fake_claude_calls(&out);
    assert_eq!(calls.len(), 1, "{calls:?}");
    let run = &calls[0];
    assert_eq!(
        run.pid, pid,
        "claude replaced tagteam: one process, one pid"
    );
    assert_eq!(
        run.args,
        [
            OsString::from("--json"),
            OsString::from("two words"),
            OsString::from_vec(b"not \xffutf-8".to_vec()),
        ]
    );
    assert_eq!(
        run.env.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("sk-ant-plain"),
        "nothing is scrubbed on the plain path (§12.5)"
    );
    assert!(!run.env.contains_key("CLAUDE_CONFIG_DIR"));
    assert!(
        output.stdout.is_empty(),
        "the agent's --json is not tagteam's"
    );
    assert!(
        !Env::for_test(root).data_dir().exists(),
        "an unmapped directory costs one start and writes nothing (§12.7)"
    );
}

#[test]
fn plain_claude_inside_a_run_shell_runs_on_the_outer_home() {
    // §12.1, §12.8, Decision 13: from a session's shell, plain `claude` gets the default home
    // back, not the profile the shell names.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    seed_home(&Env::for_test(root));
    let bin = fake_claude(root);
    let (_profile, spelling) = cc_profile(root, "0192-shell");
    let out = root.join("calls");
    let mut c = std_cmd(root);
    c.env("PATH", path_with(&bin))
        .env("FAKE_CLAUDE_OUT", &out)
        .env("CLAUDE_CONFIG_DIR", &spelling)
        .args(["run", "--", "x"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = c.spawn().unwrap();
    let pid = child.id();
    let output = finish(child, LONG);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let calls = fake_claude_calls(&out);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].pid, pid, "exec'd in place");
    assert!(
        !calls[0].env.contains_key("CLAUDE_CONFIG_DIR"),
        "the outer home defined none, so neither does plain claude"
    );
}

#[test]
fn a_missing_launch_command_fails_before_anything_is_written() {
    // §12.1: looked up on PATH before any lock; missing → exit 1, nothing changed.
    let home = Home::new();
    let output = std_cmd(home.root())
        .env("PATH", "/usr/bin:/bin")
        .current_dir(home.root().join("work"))
        .args(["--json", "run", "1", "--", "x"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(kind(&output.stdout), "launch-command-missing");
    assert!(!home.profile().exists(), "no profile, no reservation");
}

// ---- `--json` and the command line (§12.1, Decision 8, B.36) ----

#[test]
fn errors_before_the_launch_are_one_json_object_and_launch_nothing() {
    let home = Home::new();
    let out = home.out("never");
    let cases: [(&[&str], &str); 3] = [
        (&["--json", "run", "9", "--", "x"], "no-such-account"),
        (
            &["--json", "run", "--require-session", "--", "x"],
            "requires-session",
        ),
        (
            &["--json", "run", "--require-session", "2", "--", "x"],
            "requires-session",
        ),
    ];
    for (args, expected) in cases {
        let output = home.tagteam(&out).args(args).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{args:?}: {}",
            stderr(&output)
        );
        let v: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(v["schemaVersion"], json!(1), "{args:?}");
        assert_eq!(v["error"]["type"], json!(expected), "{args:?}");
    }
    assert!(
        fake_claude_calls(&out).is_empty(),
        "claude never ran, not even its login check"
    );
    assert!(!home.profile().exists());
}

#[test]
fn an_agent_s_json_after_the_double_dash_never_turns_a_usage_error_into_json() {
    // `main_with_args` reads `--json` before clap does, for a usage error; it stops at `--`.
    let d = tempfile::tempdir().unwrap();
    let out = cmd(d.path())
        .args(["run", "--bogus", "--", "--json"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(stderr(&out).starts_with("error: "), "{}", stderr(&out));
    // Before the double dash it is tagteam's flag, and the same error is one JSON object.
    let out = cmd(d.path())
        .args(["run", "--bogus", "--json", "--", "x"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert_eq!(kind(&out.stdout), "usage");
}

// ---- The session: environment, arguments, exit code (§12.5) ----

#[test]
fn the_session_gets_its_arguments_verbatim_and_its_own_environment() {
    let home = Home::new();
    let out = home.out("session");
    let output = home
        .tagteam(&out)
        .args([
            "run",
            "1",
            "--",
            "--json",
            "-p",
            "two words",
            "",
            "--",
            "ünï",
        ])
        .env("ANTHROPIC_API_KEY", "sk-ant-x")
        .env("CLAUDE_CODE_OAUTH_TOKEN", "oat")
        .env("CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR", "3")
        .env("CLAUDE_SECURESTORAGE_CONFIG_DIR", "")
        .env("KEEP_ME", "kept")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    let run = session(&out).expect("the session ran");
    assert_eq!(run.args, ["--json", "-p", "two words", "", "--", "ünï"]);
    let spelling = marker_spelling(&home.profile());
    // `CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR` is one of `CLAUDE_CODE_*_FILE_DESCRIPTOR`, which the
    // process boundary records by its name as it finds it set (Decision 18).
    let scrubbed = [
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
    ];
    let checks = login_checks(&out);
    assert_eq!(
        checks.len(),
        1,
        "one check, at the bootstrap (§12.3 step 8): {checks:?}"
    );
    let cwd = fs::canonicalize(home.root().join("work")).unwrap();
    for (what, call) in [("the session", &run), ("its login check", &checks[0])] {
        assert_eq!(call.env.get("CLAUDE_CONFIG_DIR"), Some(&spelling), "{what}");
        for gone in scrubbed {
            assert!(
                !call.env.contains_key(gone),
                "{what}: {gone} is scrubbed (§12.5)"
            );
        }
        assert_eq!(
            call.env.get("KEEP_ME").map(String::as_str),
            Some("kept"),
            "{what}"
        );
        assert_eq!(call.cwd, cwd, "{what}: where claude runs, as it runs");
    }
    let err = stderr(&output);
    for name in scrubbed {
        assert_eq!(
            err.matches(name).count(),
            1,
            "one warning names {name}:\n{err}"
        );
    }
}

#[test]
fn run_exits_with_the_session_s_code() {
    let home = Home::new();
    let out = home.out("seven");
    let output = home
        .tagteam(&out)
        .args(["run", "1", "--", "x"])
        .env("FAKE_CLAUDE_EXIT", "7")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7), "{}", stderr(&output));
    assert!(
        reservations(&home.profile()).is_empty(),
        "exit handling ran to its unlink"
    );
}

#[test]
fn a_session_killed_by_a_signal_exits_128_plus_the_signal() {
    let home = Home::new();
    let out = home.out("killed");
    let hold = home.root().join("hold");
    let _release = Release(vec![hold.clone()]);
    let mut child = start(&home, &out, &hold, &[]);
    wait_while_running(&mut child, "the session", || started(&out));
    send(session(&out).unwrap().pid, libc::SIGKILL);
    let output = finish(child, LONG);
    assert_eq!(
        output.status.code(),
        Some(128 + libc::SIGKILL),
        "{}",
        stderr(&output)
    );
    assert!(reservations(&home.profile()).is_empty());
}

#[test]
fn a_target_that_became_the_live_login_during_the_launch_runs_plain_claude_after_one_more_plan() {
    // Decision 14, B.47: `launch` decides again under its locks; `run` plans once more.
    let home = Home::new();
    let out = home.out("raced");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(vec![pause.join("resume")]);
    let mut c = home.tagteam(&out);
    c.args(["run", "1", "--", "x"])
        .env("TAGTEAM_TEST_PAUSE_AT", "launch-before-locks")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut child = spawn(c);
    let pid = child.id();
    wait_while_running(&mut child, "the launch", || pause.join("paused").exists());
    cmd(home.root()).args(["switch", "1"]).assert().success();
    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child, LONG);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("became the live login"),
        "{}",
        stderr(&output)
    );
    let calls = fake_claude_calls(&out);
    assert_eq!(
        calls.len(),
        1,
        "no login check, one plain claude: {calls:?}"
    );
    assert_eq!(calls[0].pid, pid, "exec'd in place");
    assert!(!calls[0].env.contains_key("CLAUDE_CONFIG_DIR"));
    assert!(!home.profile().exists(), "no session was started");
}

#[test]
fn a_mapped_target_removed_before_the_launch_reads_it_runs_plain_claude_after_one_more_plan() {
    // Decision 14, B.47: a `remove` that lands after `plan_run`, before the launch's first read
    // of the account, is `TargetChanged`; the mapping went with the account (Decision 7), so the
    // second plan runs plain `claude`.
    let home = Home::new();
    let out = home.out("removed");
    cmd(home.root())
        .current_dir(home.root().join("work"))
        .args(["map", "1"])
        .assert()
        .success();
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(vec![pause.join("resume")]);
    let mut c = home.tagteam(&out);
    c.args(["run", "--", "x"])
        .env("TAGTEAM_TEST_PAUSE_AT", "launch-before-freshen")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut child = spawn(c);
    let pid = child.id();
    wait_while_running(&mut child, "the launch", || pause.join("paused").exists());
    cmd(home.root()).args(["remove", "1"]).assert().success();
    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child, LONG);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("was removed"),
        "warned once: {}",
        stderr(&output)
    );
    let calls = fake_claude_calls(&out);
    assert_eq!(
        calls.len(),
        1,
        "no login check, one plain claude: {calls:?}"
    );
    assert_eq!(calls[0].pid, pid, "exec'd in place");
    assert!(!calls[0].env.contains_key("CLAUDE_CONFIG_DIR"));
    assert!(!home.profile().exists(), "no session was started");
}

#[test]
fn a_target_that_changes_again_under_the_second_plan_s_launch_is_the_command_s_error() {
    // Decision 14: `run` plans once more after a `TargetChanged`, and a second one is its error,
    // never a loop. `work` maps to b, the live login, and `work/app` to a. While the first launch
    // waits for its locks, a becomes the live login and `work/app` is unmapped, so the second
    // plan starts a session of b; while that launch waits, b becomes the live login again.
    let home = Home::new();
    let out = home.out("raced twice");
    let (work, app) = (home.root().join("work"), home.root().join("work/app"));
    fs::create_dir_all(&app).unwrap();
    let map = |dir: &Path, position: &str| {
        cmd(home.root())
            .current_dir(dir)
            .args(["map", position])
            .assert()
            .success();
    };
    map(&work, "2");
    map(&app, "1");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(vec![pause.join("resume-1"), pause.join("resume-2")]);
    let mut c = home.tagteam(&out);
    c.args(["--json", "run", "--", "x"])
        .current_dir(&app)
        .env("TAGTEAM_TEST_PAUSE_AT", "launch-before-locks")
        .env("TAGTEAM_TEST_PAUSE_EACH", "1")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut child = spawn(c);
    wait_while_running(&mut child, "the first launch", || {
        pause.join("paused-1").exists()
    });
    cmd(home.root()).args(["switch", "1"]).assert().success();
    cmd(home.root())
        .current_dir(&app)
        .arg("unmap")
        .assert()
        .success();
    fs::write(pause.join("resume-1"), b"").unwrap();
    wait_while_running(&mut child, "the second launch", || {
        pause.join("paused-2").exists()
    });
    cmd(home.root()).args(["switch", "2"]).assert().success();
    fs::write(pause.join("resume-2"), b"").unwrap();
    let output = finish(child, LONG);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(kind(&output.stdout), "target-changed");
    let v: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        v["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("position 2 became the live login")),
        "{v}"
    );
    assert_eq!(
        stderr(&output)
            .matches("warning: position 1 became the live login")
            .count(),
        1,
        "the first change is a warning, once: {}",
        stderr(&output)
    );
    assert!(fake_claude_calls(&out).is_empty(), "nothing ran");
    assert!(!home.profile().exists(), "no session was started");
}

// ---- Signals while claude runs (§12.5, Review Focus 2) ----

#[test]
fn sigterm_and_sighup_to_tagteam_reach_the_session_once_and_exit_handling_still_runs() {
    for signal in [libc::SIGTERM, libc::SIGHUP] {
        let home = Home::new();
        let out = home.out("forwarded");
        let hold = home.root().join("hold");
        let _release = Release(vec![hold.clone()]);
        let mut child = start(&home, &out, &hold, &[("FAKE_CLAUDE_ON_SIGNAL", "continue")]);
        wait_while_running(&mut child, "the session", || started(&out));
        settle();

        send(child.id(), signal);
        wait_while_running(&mut child, "the forwarded signal", || {
            caught(&out) == [signal]
        });
        settle();
        assert_eq!(caught(&out), [signal], "forwarded once, never again");
        assert!(
            child.try_wait().unwrap().is_none(),
            "{signal}: tagteam waits on"
        );

        release(&hold);
        let output = finish(child, LONG);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{signal}: the child's code: {}",
            stderr(&output)
        );
        assert!(
            !stderr(&output).contains("interrupted"),
            "{signal}: {}",
            stderr(&output)
        );
        assert!(
            reservations(&home.profile()).is_empty(),
            "{signal}: exit handling ran"
        );
        assert!(
            !home.profile().join(BASELINE).exists(),
            "{signal}: and merged back"
        );
    }
}

#[test]
fn a_forwarded_sigterm_that_ends_the_session_exits_143() {
    let home = Home::new();
    let out = home.out("term");
    let hold = home.root().join("hold");
    let _release = Release(vec![hold.clone()]);
    let mut child = start(&home, &out, &hold, &[]);
    wait_while_running(&mut child, "the session", || started(&out));
    settle();
    send(child.id(), libc::SIGTERM);
    let output = finish(child, LONG);
    assert_eq!(
        output.status.code(),
        Some(128 + libc::SIGTERM),
        "{}",
        stderr(&output)
    );
    assert!(reservations(&home.profile()).is_empty());
}

#[test]
fn sigint_to_tagteam_alone_never_reaches_the_session() {
    // Only the terminal sends claude a Ctrl-C (it shares the foreground group); tagteam drops
    // its own copy.
    let home = Home::new();
    let out = home.out("int");
    let hold = home.root().join("hold");
    let _release = Release(vec![hold.clone()]);
    let mut child = start(&home, &out, &hold, &[("FAKE_CLAUDE_ON_SIGNAL", "continue")]);
    wait_while_running(&mut child, "the session", || started(&out));
    settle();
    send(child.id(), libc::SIGINT);
    settle();
    settle();
    assert!(caught(&out).is_empty(), "nothing forwarded");
    assert!(
        child.try_wait().unwrap().is_none(),
        "and tagteam ignored it"
    );
    release(&hold);
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        !stderr(&output).contains("interrupted"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn ctrl_c_at_the_terminal_reaches_the_session_once_and_tagteam_ignores_it() {
    let home = Home::new();
    let out = home.out("ctrl-c");
    let hold = home.root().join("hold");
    let _release = Release(vec![hold.clone()]);
    let mut child = start(&home, &out, &hold, &[("FAKE_CLAUDE_ON_SIGNAL", "continue")]);
    wait_while_running(&mut child, "the session", || started(&out));
    settle();

    send_group(child.id(), libc::SIGINT);
    wait_while_running(&mut child, "the Ctrl-C", || caught(&out) == [libc::SIGINT]);
    settle();
    assert_eq!(
        caught(&out),
        [libc::SIGINT],
        "once: the terminal's own, never forwarded on top"
    );
    assert!(child.try_wait().unwrap().is_none());
    release(&hold);
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        !stderr(&output).contains("interrupted"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn sigquit_never_stops_tagteam() {
    let home = Home::new();
    let out = home.out("quit");
    let hold = home.root().join("hold");
    let _release = Release(vec![hold.clone()]);
    let mut child = start(&home, &out, &hold, &[]);
    wait_while_running(&mut child, "the session", || started(&out));
    settle();
    send(child.id(), libc::SIGQUIT);
    settle();
    assert!(
        child.try_wait().unwrap().is_none(),
        "tagteam survives Ctrl-\\ while claude runs (Decision 1)"
    );
    release(&hold);
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

#[test]
fn ctrl_backslash_at_the_terminal_ends_the_session_and_tagteam_reports_its_code() {
    // claude keeps Ctrl-\'s default action, which a handler in tagteam does not take from it
    // (an ignored signal would): it dies of it, and tagteam lives to exit 131. The fake leaves
    // SIGQUIT untrapped, as Claude Code does.
    let home = Home::new();
    let out = home.out("ctrl-backslash");
    let hold = home.root().join("hold");
    let _release = Release(vec![hold.clone()]);
    let mut child = start(&home, &out, &hold, &[]);
    wait_while_running(&mut child, "the session", || started(&out));
    settle();
    send_group(child.id(), libc::SIGQUIT);
    let output = finish(child, LONG);
    assert_eq!(
        output.status.code(),
        Some(128 + libc::SIGQUIT),
        "{}",
        stderr(&output)
    );
    assert!(
        reservations(&home.profile()).is_empty(),
        "exit handling ran"
    );
}

// ---- Before the spawn (§12.5 "Signals", §15.2 "Exit paths") ----

#[test]
fn a_signal_while_the_launch_waits_for_its_lock_launches_nothing() {
    let home = Home::new();
    let out = home.out("blocked");
    let guard =
        MutationGuard::acquire(&Env::for_test(home.root()), Duration::from_secs(5)).unwrap();
    let mut c = home.tagteam(&out);
    c.args(["--json", "run", "1", "--", "x"]);
    let mut child = spawn(c);
    // Into the launch's 30 s wait for the mutation lock (§9.1).
    thread::sleep(Duration::from_millis(500));
    assert!(child.try_wait().unwrap().is_none(), "it waits for the lock");
    send(child.id(), libc::SIGINT);
    let output = finish(child, Duration::from_secs(5));
    drop(guard);

    assert_eq!(output.status.code(), Some(130), "{}", stderr(&output));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "interrupted", "message": "interrupted"}})
    );
    assert!(
        fake_claude_calls(&out).is_empty(),
        "nothing launched, not even a login check"
    );
    assert!(reservations(&home.profile()).is_empty());
}

#[test]
fn a_signal_during_the_login_check_launches_nothing_and_still_runs_exit_handling() {
    let home = Home::new();
    // The first launch bootstraps, and its check runs under the locks. The second checks
    // after its reservation exists (§12.3 "Every launch is checked").
    let first = home.out("first");
    let status = home
        .tagteam(&first)
        .args(["run", "1", "--", "x"])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0));
    let out = home.out("second");
    let mut c = home.tagteam(&out);
    c.args(["--json", "run", "1", "--", "x"])
        .env("FAKE_CLAUDE_AUTH_SLEEP", "20");
    let mut child = spawn(c);
    wait_while_running(&mut child, "the login check", || {
        !login_checks(&out).is_empty()
    });
    send(child.id(), libc::SIGTERM);
    let output = finish(child, Duration::from_secs(5));

    assert_eq!(
        output.status.code(),
        Some(128 + libc::SIGTERM),
        "{}",
        stderr(&output)
    );
    assert_eq!(kind(&output.stdout), "interrupted");
    assert!(session(&out).is_none(), "claude never started");
    assert!(
        reservations(&home.profile()).is_empty(),
        "its exit handling ran, as for a refused launch, past the signal that ended it"
    );
    // Controller ruling (Task 11): the signal that ended the launch is spent on it, so exit
    // handling ran to completion rather than deferring at its first lock wait.
    assert!(
        !stderr(&output).contains("did not finish"),
        "nothing deferred: {}",
        stderr(&output)
    );
    assert!(
        !home.profile().join(BASELINE).exists(),
        "the seed's baseline was merged back"
    );
}

#[test]
fn a_signal_recorded_after_the_last_check_reaches_the_session_once_it_starts() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let home = Home::new();
        let out = home.out("late");
        let hold = home.root().join("hold");
        let pause = home.root().join("pause");
        fs::create_dir_all(&pause).unwrap();
        let _release = Release(vec![hold.clone(), pause.join("resume")]);
        let mut child = start(
            &home,
            &out,
            &hold,
            &[
                ("FAKE_CLAUDE_ON_SIGNAL", "continue"),
                ("TAGTEAM_TEST_PAUSE_AT", "before-spawn"),
                ("TAGTEAM_TEST_PAUSE_DIR", pause.to_str().unwrap()),
            ],
        );
        wait_while_running(&mut child, "the spawn", || pause.join("paused").exists());
        send(child.id(), signal);
        fs::write(pause.join("resume"), b"").unwrap();

        // The fake records it when its trap was set in time, or dies of it when it was not:
        // either way the signal reached claude, not tagteam's own interruption.
        let deadline = Instant::now() + LONG;
        let recorded = loop {
            if child.try_wait().unwrap().is_some() {
                break false;
            }
            if caught(&out) == [signal] {
                break true;
            }
            assert!(
                Instant::now() < deadline,
                "{signal} never reached the session"
            );
            thread::sleep(Duration::from_millis(10));
        };
        if recorded {
            settle();
            assert_eq!(caught(&out), [signal], "{signal}: forwarded once");
        }
        release(&hold);
        let output = finish(child, LONG);
        let expected = if recorded { 0 } else { 128 + signal };
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{signal}: {}",
            stderr(&output)
        );
        assert!(
            !stderr(&output).contains("interrupted"),
            "past the last check a signal is claude's: {}",
            stderr(&output)
        );
        assert!(reservations(&home.profile()).is_empty(), "{signal}");
    }
}

#[test]
fn a_launch_command_gone_at_the_spawn_refuses_and_runs_exit_handling() {
    let home = Home::new();
    let out = home.out("gone");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(vec![pause.join("resume")]);
    let mut c = home.tagteam(&out);
    c.args(["--json", "run", "1", "--", "x"])
        .env("TAGTEAM_TEST_PAUSE_AT", "before-spawn")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut child = spawn(c);
    wait_while_running(&mut child, "the spawn", || pause.join("paused").exists());
    fs::remove_file(home.root().join("bin/claude")).unwrap();
    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child, LONG);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(kind(&output.stdout), "launch-unreachable");
    assert!(session(&out).is_none());
    assert!(
        reservations(&home.profile()).is_empty(),
        "a launch refused after its reservation exists runs its exit handling (§12.3)"
    );
}

#[test]
fn a_signal_pending_when_the_spawn_fails_is_spent_on_the_exit_handling_and_never_too_late() {
    // `abandon` takes the token whatever refused the launch. A signal recorded after the token's
    // last look arrived after the decision: the refusal stands, its exit handling runs to
    // completion, and nothing is reported as too late.
    let home = Home::new();
    let out = home.out("gone-signalled");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _release = Release(vec![pause.join("resume")]);
    let mut c = home.tagteam(&out);
    c.args(["--json", "run", "1", "--", "x"])
        .env("TAGTEAM_TEST_PAUSE_AT", "before-spawn")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut child = spawn(c);
    wait_while_running(&mut child, "the spawn", || pause.join("paused").exists());
    send(child.id(), libc::SIGTERM);
    fs::remove_file(home.root().join("bin/claude")).unwrap();
    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child, LONG);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(
        kind(&output.stdout),
        "launch-unreachable",
        "the refusal stands"
    );
    assert!(session(&out).is_none());
    assert!(!stderr(&output).contains("too late"), "{}", stderr(&output));
    assert!(
        !stderr(&output).contains("did not finish"),
        "nothing deferred: {}",
        stderr(&output)
    );
    assert!(
        reservations(&home.profile()).is_empty(),
        "exit handling ran"
    );
    assert!(
        !home.profile().join(BASELINE).exists(),
        "the bootstrap's baseline was merged back"
    );
}

// ---- Task 13: the kill paths, two sessions, and the races (§15.2, Review Focus 1 and 3) ----

/// The token endpoint's path under a test base (Appendix A.5).
const TOKEN: &str = "/v1/oauth/token";

impl Home {
    /// The binary for a command that runs no agent.
    fn cmd(&self) -> assert_cmd::Command {
        cmd(self.root())
    }
}

/// Polls `ready` every 10 ms until it holds; fails after `within`. (Task 3's `wait_for` polls
/// one `FAKE_CLAUDE_OUT` file; this one waits on anything.)
fn wait_until(within: Duration, what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + within;
    while !ready() {
        assert!(Instant::now() < deadline, "never got to {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// A Claude Code credential of `rt`'s lineage, for `FAKE_CLAUDE_ROTATE`: what the session's
/// own refresh writes to its profile (Appendix A.4's account-scoped keys).
fn rotated(rt: &str) -> String {
    json!({"claudeAiOauth": {
        "accessToken": format!("at-{rt}"),
        "refreshToken": rt,
        "expiresAt": 4_102_444_800_000i64,
        "refreshTokenExpiresAt": 4_102_444_800_000i64,
        "scopes": ["user:inference", "user:profile"]
    }})
    .to_string()
}

/// The refresh token in a credential's bytes.
fn refresh_token(bytes: &[u8]) -> String {
    let v: Value = serde_json::from_slice(bytes).unwrap();
    v["claudeAiOauth"]["refreshToken"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The refresh token of `id`'s vault generation (§6.2).
fn vault_rt(root: &Path, id: &str) -> String {
    let bytes = FileKeychain::new(root.join("keychain"))
        .find(SERVICE, id)
        .present()
        .expect("a vault generation");
    refresh_token(&bytes)
}

/// The refresh token of the default home's live credential, as Claude Code reads it.
fn live_rt(root: &Path) -> String {
    let env = Env::for_test(root);
    let bytes = FileKeychain::new(root.join("keychain"))
        .find(
            &keychain_service(&env, ItemKind::OAuth),
            &keychain_account(&env),
        )
        .present()
        .expect("a live credential");
    refresh_token(&bytes)
}

/// The refresh token in `profile`'s credential file.
fn profile_rt(profile: &Path) -> String {
    refresh_token(&fs::read(profile.join(".credentials.json")).unwrap())
}

/// What a session does to its own config (§12.4): it trusts the project at `path`.
fn trust(profile: &Path, path: &str) {
    let file = profile.join(".claude.json");
    let doc = fs::read(&file).unwrap();
    let mut projects = get_top_level(&doc, "projects")
        .unwrap()
        .unwrap_or_else(|| json!({}));
    projects[path] = json!({"allowedTools": [], "hasTrustDialogAccepted": true});
    fs::write(
        &file,
        replace_top_level(&doc, "projects", &projects).unwrap(),
    )
    .unwrap();
}

/// Whether the config at `file` trusts the project at `path`.
fn trusts(file: &Path, path: &str) -> bool {
    let doc = fs::read(file).unwrap();
    get_top_level(&doc, "projects")
        .unwrap()
        .is_some_and(|p| p[path]["hasTrustDialogAccepted"] == json!(true))
}

/// The default home's `~/.claude.json`.
fn default_config(home: &Home) -> PathBuf {
    home.root().join("home/.claude.json")
}

/// No reservation file in `profile` is held: whatever held one has exited (§12.5).
fn all_free(profile: &Path) -> bool {
    reservations(profile)
        .iter()
        .all(|r| probe_lock(r).unwrap() != LockProbe::Held)
}

/// `list --json`'s row for account `id`.
fn listed(v: &Value, id: &str) -> Value {
    v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(id))
        .cloned()
        .unwrap()
}

/// `tagteam <args> --json` for a command that refuses: exit 1, and the error's kind.
fn refused(home: &Home, args: &[&str]) -> String {
    let out = home.cmd().args(args).arg("--json").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
    kind(&out.stdout)
}

/// Account `a` is in a session, as another process sees it: `list` marks it, sending no token
/// request, and `switch` and `remove` refuse (§10.3, §12.5).
fn assert_session_owned(home: &Home, server: &MockServer) {
    let out = home
        .cmd()
        .env("TAGTEAM_TEST_API_BASE", server.base_url())
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listed(&v, &home.a)["inSession"], json!(true), "{v}");
    assert_eq!(
        server.hits("POST", TOKEN),
        0,
        "the gate never refreshes a session's account"
    );
    assert_eq!(refused(home, &["switch", "1"]), "session-owned");
    assert_eq!(live_email(home.root()), "b@x.co");
    assert_eq!(refused(home, &["remove", "1"]), "session-owned");
    assert!(home.profile().exists());
}

/// Review Focus 1's start: account `a`'s session runs, rotates its credential to `rt-a2`, and
/// trusts `/work/killed`; then `tagteam` is killed with SIGKILL under it. Returns the hold file
/// that lets the orphaned session go, and its guard.
fn kill_mid_session(home: &Home) -> (PathBuf, Release) {
    let out = home.out("killed");
    let hold = home.root().join("hold-killed");
    let guard = Release(vec![hold.clone()]);
    let mut c = home.tagteam(&out);
    c.args(["run", "1", "--", "x"])
        .env("FAKE_CLAUDE_HOLD", &hold)
        .env("FAKE_CLAUDE_ROTATE", rotated("rt-a2"))
        .process_group(0)
        .stdin(Stdio::null())
        // The orphaned session keeps tagteam's output: never a pipe this test reads.
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = c.spawn().unwrap();
    wait_while_running(&mut child, "the session", || started(&out));
    trust(&home.profile(), "/work/killed");
    send(child.id(), libc::SIGKILL);
    assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));
    (hold, guard)
}

/// After `kill_mid_session`: `claude` holds the reservation through its inherited fd, so the
/// account stays session-owned, and nothing is captured under it (§12.5, B.46).
fn assert_still_owned(home: &Home) {
    // Due, so the gate would refresh `a` if it were not in a session: the zero token requests
    // `assert_session_owned` counts mean something.
    expire_vault(home.root(), &home.a, 60_000);
    let held = reservations(&home.profile());
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(probe_lock(&held[0]).unwrap(), LockProbe::Held);
    assert_session_owned(home, &MockServer::start());
    assert_eq!(
        vault_rt(home.root(), &home.a),
        "rt-a",
        "nothing captured under a session"
    );
}

#[test]
fn a_killed_tagteam_leaves_its_reservation_live_and_the_next_launch_captures_and_merges_back() {
    let home = Home::new();
    let profile = home.profile();
    let (hold, _guard) = kill_mid_session(&home);
    assert_still_owned(&home);

    // A launch now joins the orphaned session: no capture, no bootstrap and no seed under it.
    let join = home.out("join");
    let out = home
        .tagteam(&join)
        .args(["run", "1", "--", "x"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a");
    assert_eq!(
        profile_rt(&profile),
        "rt-a2",
        "never bootstrapped over (B.28)"
    );
    assert!(
        trusts(&profile.join(".claude.json"), "/work/killed"),
        "never re-seeded (B.44)"
    );
    assert!(profile.join(BASELINE).exists());
    assert!(
        !trusts(&default_config(&home), "/work/killed"),
        "not merged while it runs"
    );

    // The orphan exits, and its lock goes with it.
    release(&hold);
    wait_until(LONG, "the orphaned session's exit", || all_free(&profile));
    // §12.5 step 2: the next launch captures and merges back before it spawns, so both are
    // done by the time it parks at `before-spawn`, ahead of its own exit handling.
    let next = home.out("next");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _resume = Release(vec![pause.join("resume")]);
    let mut c = home.tagteam(&next);
    c.args(["run", "1", "--", "x"])
        .env("TAGTEAM_TEST_PAUSE_AT", "before-spawn")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut child = spawn(c);
    wait_while_running(&mut child, "the spawn", || pause.join("paused").exists());
    assert_eq!(
        vault_rt(home.root(), &home.a),
        "rt-a2",
        "lazily captured at the next launch, before it spawns"
    );
    assert!(
        trusts(&default_config(&home), "/work/killed"),
        "the left-over baseline merged back first (§12.5 step 2), before this launch's exit"
    );
    fs::write(pause.join("resume"), b"").unwrap();
    let out = finish(child, LONG);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a2");
    assert!(trusts(&default_config(&home), "/work/killed"));
    assert!(!profile.join(BASELINE).exists());
    assert!(
        reservations(&profile).is_empty(),
        "the dead reservation is gone too"
    );
}

#[test]
fn after_a_killed_tagteam_s_session_ends_a_switch_captures_its_rotation_first() {
    let home = Home::new();
    let profile = home.profile();
    let (hold, _guard) = kill_mid_session(&home);
    assert_eq!(refused(&home, &["switch", "1"]), "session-owned");

    release(&hold);
    wait_until(LONG, "the orphaned session's exit", || all_free(&profile));
    let out = home.cmd().args(["switch", "1", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        vault_rt(home.root(), &home.a),
        "rt-a2",
        "§9.2's pre-check captured it"
    );
    assert_eq!(
        live_rt(home.root()),
        "rt-a2",
        "and the switch activated it, never the consumed rt-a"
    );
    assert!(
        profile.join(BASELINE).exists(),
        "the merge-back waits for the next launch"
    );
}

#[test]
fn after_a_killed_tagteam_s_session_ends_the_gate_captures_its_rotation() {
    let home = Home::new();
    let profile = home.profile();
    let (hold, _guard) = kill_mid_session(&home);
    release(&hold);
    wait_until(LONG, "the orphaned session's exit", || all_free(&profile));

    // §7.3 step 3: the next refresh `a` needs, which `list`'s collection reaches.
    expire_vault(home.root(), &home.a, 60_000);
    home.cmd().args(["list", "--json"]).assert().success();
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a2");
}

#[test]
fn the_last_of_two_sessions_out_captures_a_rotation_either_made_and_merges_back() {
    // Review Focus 3, in both orders. The session that rotates also exits first, so it is
    // always the other, last one out that captures.
    for first_rotates in [true, false] {
        let home = Home::new();
        let profile = home.profile();
        let (o1, o2) = (home.out("one"), home.out("two"));
        let (h1, h2) = (home.root().join("hold-one"), home.root().join("hold-two"));
        let _guard = Release(vec![h1.clone(), h2.clone()]);
        let rotation = rotated("rt-a2");
        let rotate: &[(&str, &str)] = &[("FAKE_CLAUDE_ROTATE", rotation.as_str())];

        let mut one = start(&home, &o1, &h1, if first_rotates { rotate } else { &[] });
        wait_while_running(&mut one, "the first session", || started(&o1));
        trust(&profile, "/work/shared");
        let baseline = fs::read(profile.join(BASELINE)).unwrap();
        let mut two = start(&home, &o2, &h2, if first_rotates { &[] } else { rotate });
        wait_while_running(&mut two, "the second session", || started(&o2));

        // The second joined: no seed over the first's changes, no bootstrap over its credential.
        assert!(
            trusts(&profile.join(".claude.json"), "/work/shared"),
            "{first_rotates}"
        );
        assert_eq!(
            fs::read(profile.join(BASELINE)).unwrap(),
            baseline,
            "{first_rotates}"
        );
        assert_eq!(profile_rt(&profile), "rt-a2", "{first_rotates}");
        assert_eq!(reservations(&profile).len(), 2, "{first_rotates}");

        // The first out leaves capture and merge-back to the last.
        let (first, first_hold, last, last_hold) = if first_rotates {
            (one, h1.clone(), two, h2.clone())
        } else {
            (two, h2.clone(), one, h1.clone())
        };
        release(&first_hold);
        let out = finish(first, LONG);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{first_rotates}: {}",
            stderr(&out)
        );
        assert_eq!(vault_rt(home.root(), &home.a), "rt-a", "{first_rotates}");
        assert!(
            !trusts(&default_config(&home), "/work/shared"),
            "{first_rotates}"
        );
        assert!(profile.join(BASELINE).exists(), "{first_rotates}");
        assert_eq!(reservations(&profile).len(), 1, "{first_rotates}");

        release(&last_hold);
        let out = finish(last, LONG);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{first_rotates}: {}",
            stderr(&out)
        );
        assert_eq!(vault_rt(home.root(), &home.a), "rt-a2", "{first_rotates}");
        assert!(
            trusts(&default_config(&home), "/work/shared"),
            "{first_rotates}"
        );
        assert!(!profile.join(BASELINE).exists(), "{first_rotates}");
        assert!(reservations(&profile).is_empty(), "{first_rotates}");
    }
}

#[test]
fn a_reservation_owns_the_account_before_claude_starts() {
    // §12.5: the reservation covers the gap before claude writes its own session record.
    let home = Home::new();
    let server = MockServer::start();
    // Due, so the gate would refresh `a` if it were not in a session.
    expire_vault(home.root(), &home.a, 60_000);
    let out = home.out("racing");
    let hold = home.root().join("hold");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _guard = Release(vec![hold.clone(), pause.join("resume")]);
    let mut child = start(
        &home,
        &out,
        &hold,
        &[
            ("TAGTEAM_TEST_PAUSE_AT", "before-spawn"),
            ("TAGTEAM_TEST_PAUSE_DIR", pause.to_str().unwrap()),
        ],
    );
    wait_while_running(&mut child, "the spawn", || pause.join("paused").exists());
    assert!(session(&out).is_none(), "claude has not started");
    assert_eq!(reservations(&home.profile()).len(), 1);

    assert_session_owned(&home, &server);

    fs::write(pause.join("resume"), b"").unwrap();
    wait_while_running(&mut child, "the session", || started(&out));
    release(&hold);
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

#[test]
fn exit_handling_finishes_before_another_process_can_take_the_account() {
    // §12.5 "When the child exits": between claude's exit and the unlink, the reservation is
    // still tagteam's, so remove, switch and the gate all see a session.
    let home = Home::new();
    let server = MockServer::start();
    expire_vault(home.root(), &home.a, 60_000);
    let profile = home.profile();
    let out = home.out("exiting");
    let hold = home.root().join("hold");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _guard = Release(vec![hold.clone(), pause.join("resume")]);
    let rotation = rotated("rt-a2");
    let mut child = start(
        &home,
        &out,
        &hold,
        &[
            ("FAKE_CLAUDE_ROTATE", rotation.as_str()),
            ("TAGTEAM_TEST_PAUSE_AT", "after-exit"),
            ("TAGTEAM_TEST_PAUSE_DIR", pause.to_str().unwrap()),
        ],
    );
    wait_while_running(&mut child, "the session", || started(&out));
    trust(&profile, "/work/raced");
    release(&hold);
    wait_while_running(&mut child, "the exit handling", || {
        pause.join("paused").exists()
    });
    assert!(
        session(&out).is_some_and(|c| c.exit.is_some()),
        "claude has exited"
    );

    assert_session_owned(&home, &server);
    assert_eq!(
        vault_rt(home.root(), &home.a),
        "rt-a",
        "nothing captured outside exit handling"
    );

    fs::write(pause.join("resume"), b"").unwrap();
    let output = finish(child, LONG);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a2");
    assert!(trusts(&default_config(&home), "/work/raced"));
    assert!(!profile.join(BASELINE).exists());
    assert!(reservations(&profile).is_empty());
    // Quiescent now: remove goes ahead and takes the profile with it.
    home.cmd().args(["remove", "1"]).assert().success();
    assert!(!profile.exists());
}

#[test]
fn a_signal_during_exit_handling_defers_it_to_the_next_launch_and_keeps_the_child_s_code() {
    // §12.5 "After claude exits", B.63, §15.2 "Exit paths".
    let home = Home::new();
    let profile = home.profile();
    let out = home.out("deferred");
    let hold = home.root().join("hold");
    let pause = home.root().join("pause");
    fs::create_dir_all(&pause).unwrap();
    let _guard = Release(vec![hold.clone(), pause.join("resume")]);
    let rotation = rotated("rt-a2");
    let mut child = start(
        &home,
        &out,
        &hold,
        &[
            ("FAKE_CLAUDE_ROTATE", rotation.as_str()),
            ("FAKE_CLAUDE_EXIT", "3"),
            ("TAGTEAM_TEST_PAUSE_AT", "after-exit"),
            ("TAGTEAM_TEST_PAUSE_DIR", pause.to_str().unwrap()),
        ],
    );
    wait_while_running(&mut child, "the session", || started(&out));
    trust(&profile, "/work/deferred");
    release(&hold);
    wait_while_running(&mut child, "the exit handling", || {
        pause.join("paused").exists()
    });
    let lock = MutationGuard::acquire(&Env::for_test(home.root()), Duration::from_secs(5)).unwrap();
    fs::write(pause.join("resume"), b"").unwrap();
    settle(); // into exit handling's wait for the mutation lock
    send(child.id(), libc::SIGTERM);
    let output = finish(child, LONG);
    drop(lock);

    assert_eq!(
        output.status.code(),
        Some(3),
        "the child's code: {}",
        stderr(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("note: "),
        "a notice says what was left undone:\n{err}"
    );
    assert!(!err.contains("too late"), "{err}");
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a");
    assert!(!trusts(&default_config(&home), "/work/deferred"));
    assert!(profile.join(BASELINE).exists());
    assert!(all_free(&profile), "what it left died with it");

    let next = home.out("next");
    let output = home
        .tagteam(&next)
        .args(["run", "1", "--", "x"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(vault_rt(home.root(), &home.a), "rt-a2");
    assert!(trusts(&default_config(&home), "/work/deferred"));
    assert!(!profile.join(BASELINE).exists());
    assert!(reservations(&profile).is_empty());
}
