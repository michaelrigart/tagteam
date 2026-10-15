//! `tagteam daemon install`, `uninstall` and `status` through the binary (D8, D9). The service
//! manager is a stand-in script on `PATH` (`install_script`) that records its arguments, so no
//! test touches this machine's launchd or systemd (§15.1). The platform is the test-support
//! override, so both units run on either OS. The one exception is D14's real round trip,
//! `#[ignore]`d and run by hand (plan B Task 10). Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::{install_script, short_tempdir, std_cmd, two_fresh_accounts};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::service::{ServiceKind, parse_unit, unit_path};
use tagteam_provider::process::current_uid;
use tagteam_provider::{Env, ProcessStamp, ProfileMarker, profile_path};

const UNATTENDED: &str = "The daemon will switch accounts unattended; pause it with `tagteam config set daemon.autoswitch false`.\n";
const LINGERING_OFF: &str = "lingering is off for tester, so systemd stops the daemon when you log out; `loginctl enable-linger tester` keeps it running";

/// `tagteam <args>` as `platform` (`macos` or `linux`), with `<root>/bin` alone on `PATH`.
fn service_cmd(root: &Path, platform: &str, args: &[&str]) -> Output {
    std_cmd(root)
        .env("TAGTEAM_TEST_PLATFORM", platform)
        .env("PATH", root.join("bin"))
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(&out.stdout)))
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// `<root>/bin`, created.
fn bin(root: &Path) -> std::path::PathBuf {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    bin
}

/// A stand-in `launchctl`, logging each call's arguments to `bin/launchctl.log`. `bootout` and
/// `print` find nothing loaded, with exit 113 as macOS reports it; anything else succeeds.
fn launchctl(bin: &Path) {
    let log = bin.join("launchctl.log");
    install_script(
        &bin.join("launchctl"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\ncase \"$1\" in\n  bootout) echo 'Boot-out failed: 113: Could not find specified service' >&2; exit 113 ;;\n  print) echo 'Could not find service \"io.github.michaelrigart.tagteam\" in domain for user gui: 501' >&2; exit 113 ;;\nesac\nexit 0\n",
            log = log.display()
        ),
    );
}

/// A stand-in `systemctl` that succeeds at everything but `stop`, which finds the unit not
/// loaded (exit 5) as systemd does once its file is gone, and a `loginctl` that reports lingering
/// off. Each logs its arguments beside itself.
fn systemd(bin: &Path) {
    let log = bin.join("systemctl.log");
    install_script(
        &bin.join("systemctl"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\ncase \"$2\" in\n  stop) echo 'Failed to stop tagteam.service: Unit tagteam.service not loaded.' >&2; exit 5 ;;\nesac\nexit 0\n",
            log = log.display()
        ),
    );
    let log = bin.join("loginctl.log");
    install_script(
        &bin.join("loginctl"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\necho Linger=no\n",
            log = log.display()
        ),
    );
}

/// The arguments of each call of `program`, one line per call.
fn calls(bin: &Path, program: &str) -> Vec<String> {
    fs::read_to_string(bin.join(format!("{program}.log")))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn install_print_shows_this_platform_s_unit_and_writes_nothing() {
    // D9: --print changes nothing. D9.1: the program is this binary (no tagteam on PATH), and
    // only the recorded variables that are set are recorded, quoting and all.
    let proxy = r#"http://us$er:p%40ss"w\rd@proxy.example:8080"#;
    for (platform, kind) in [
        ("macos", ServiceKind::Launchd),
        ("linux", ServiceKind::Systemd),
    ] {
        let d = tempfile::tempdir().unwrap();
        let bin = bin(d.path());
        let data = d.path().join("data dir ü");
        let out = std_cmd(d.path())
            .env("TAGTEAM_TEST_PLATFORM", platform)
            .env("PATH", &bin)
            .env("XDG_DATA_HOME", &data)
            .env("HTTPS_PROXY", proxy)
            .env("NOT_RECORDED", "x")
            .args(["daemon", "install", "--print"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let unit = parse_unit(kind, &text(&out.stdout)).expect("the unit tagteam renders");
        assert_eq!(
            fs::canonicalize(&unit.program).unwrap(),
            fs::canonicalize(assert_cmd::cargo::cargo_bin("tagteam")).unwrap()
        );
        assert_eq!(
            unit.env,
            vec![
                ("XDG_DATA_HOME".to_owned(), data.display().to_string()),
                ("HTTPS_PROXY".to_owned(), proxy.to_owned()),
                ("PATH".to_owned(), bin.display().to_string()),
            ]
        );
        let path = unit_path(&Env::for_test(d.path()), kind);
        assert!(!path.exists(), "{platform}: nothing written");
        assert!(!data.exists(), "{platform}: no data directory made");

        let out = service_cmd(
            d.path(),
            platform,
            &["--json", "daemon", "install", "--print"],
        );
        assert_eq!(out.status.code(), Some(0));
        let v = json_of(&out);
        assert_eq!(
            v["service"],
            json!({"kind": kind.as_str(), "path": path.display().to_string(), "label": kind.label()})
        );
        assert!(parse_unit(kind, v["unit"].as_str().unwrap()).is_some());
    }
}

#[test]
fn install_and_uninstall_drive_systemd_through_the_systemctl_on_path() {
    let d = tempfile::tempdir().unwrap();
    let bin = bin(d.path());
    systemd(&bin);
    let path = d.path().join("home/.config/systemd/user/tagteam.service");

    let out = service_cmd(d.path(), "linux", &["--json", "daemon", "install"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        json_of(&out),
        json!({"schemaVersion": 1, "ok": true,
               "service": {"kind": "systemd", "path": path.display().to_string(),
                           "label": "tagteam.service"},
               "started": true, "warnings": [LINGERING_OFF]})
    );
    assert_eq!(mode(&path), 0o600);
    assert_eq!(
        parse_unit(ServiceKind::Systemd, &fs::read_to_string(&path).unwrap())
            .unwrap()
            .env,
        vec![("PATH".to_owned(), bin.display().to_string())]
    );
    assert_eq!(
        calls(&bin, "systemctl"),
        [
            "--user daemon-reload",
            "--user enable tagteam.service",
            "--user restart tagteam.service"
        ]
    );
    assert_eq!(calls(&bin, "loginctl"), ["show-user tester -p Linger"]);

    let out = service_cmd(d.path(), "linux", &["daemon", "install", "--no-start"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        format!(
            "Installed the systemd user unit at {}; it starts at the next login.\n{UNATTENDED}",
            path.display()
        )
    );
    assert_eq!(text(&out.stderr), format!("warning: {LINGERING_OFF}\n"));
    assert_eq!(
        calls(&bin, "systemctl")[3..],
        ["--user daemon-reload", "--user enable tagteam.service"]
    );

    let out = service_cmd(d.path(), "linux", &["--json", "daemon", "uninstall"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        json_of(&out),
        json!({"schemaVersion": 1, "ok": true, "removed": true, "stopped": false})
    );
    assert!(!path.exists());
    assert_eq!(
        calls(&bin, "systemctl")[5..],
        [
            "--user disable --now tagteam.service",
            "--user daemon-reload"
        ]
    );
    let out = service_cmd(d.path(), "linux", &["daemon", "uninstall"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout),
        format!("No systemd user unit is installed at {}.\n", path.display())
    );
    assert_eq!(
        calls(&bin, "systemctl")[7..],
        ["--user stop tagteam.service"],
        "with no unit file, only a best-effort stop"
    );
}

/// A stand-in manager that succeeds at everything, `bootout` and `stop` included: the daemon's
/// unit is loaded and running whatever its file says.
fn loaded(bin: &Path, program: &str) {
    let log = bin.join(format!("{program}.log"));
    install_script(
        &bin.join(program),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\nexit 0\n",
            log = log.display()
        ),
    );
}

#[test]
fn uninstall_says_what_it_did_for_each_of_its_three_outcomes() {
    // D9.4: `Removed` is covered by the round trips above. Here the unit file is absent: a
    // loaded unit is stopped (`StoppedOnly`, `removed` and `stopped` both true), and one that
    // is not loaded leaves nothing to do (`Nothing`).
    for (platform, program, unit, noun, stopped_call) in [
        (
            "macos",
            "launchctl",
            "home/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist",
            "launchd agent",
            format!(
                "bootout gui/{}/io.github.michaelrigart.tagteam",
                current_uid()
            ),
        ),
        (
            "linux",
            "systemctl",
            "home/.config/systemd/user/tagteam.service",
            "systemd user unit",
            "--user stop tagteam.service".to_owned(),
        ),
    ] {
        let d = tempfile::tempdir().unwrap();
        let bin = bin(d.path());
        let path = d.path().join(unit);
        // Nothing loaded: the default stand-ins answer "not loaded".
        if platform == "macos" {
            launchctl(&bin);
        } else {
            systemd(&bin);
        }
        let out = service_cmd(d.path(), platform, &["--json", "daemon", "uninstall"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{platform}: {}",
            text(&out.stderr)
        );
        assert_eq!(
            json_of(&out),
            json!({"schemaVersion": 1, "ok": true, "removed": false, "stopped": false}),
            "{platform}"
        );
        let out = service_cmd(d.path(), platform, &["daemon", "uninstall"]);
        assert_eq!(
            text(&out.stdout),
            format!("No {noun} is installed at {}.\n", path.display()),
            "{platform}"
        );

        // Loaded without a file.
        loaded(&bin, program);
        let out = service_cmd(d.path(), platform, &["--json", "daemon", "uninstall"]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{platform}: {}",
            text(&out.stderr)
        );
        assert_eq!(
            json_of(&out),
            json!({"schemaVersion": 1, "ok": true, "removed": true, "stopped": true}),
            "{platform}"
        );
        let out = service_cmd(d.path(), platform, &["daemon", "uninstall"]);
        assert_eq!(
            text(&out.stdout),
            format!(
                "Stopped the daemon; no unit file was left at {}.\n",
                path.display()
            ),
            "{platform}"
        );
        assert_eq!(
            calls(&bin, program).last().map(String::as_str),
            Some(stopped_call.as_str()),
            "{platform}"
        );
        assert!(!path.exists(), "{platform}: no file was made");
    }
}

#[test]
fn install_on_macos_loads_the_agent_after_an_unload_that_finds_nothing() {
    let d = tempfile::tempdir().unwrap();
    let bin = bin(d.path());
    launchctl(&bin);
    let path = d
        .path()
        .join("home/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist");
    let out = service_cmd(d.path(), "macos", &["daemon", "install"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        format!(
            "Installed the launchd agent at {}, and started the daemon.\n{UNATTENDED}",
            path.display()
        )
    );
    assert_eq!(text(&out.stderr), "");
    assert_eq!(mode(&path), 0o600);
    let uid = current_uid();
    assert_eq!(
        calls(&bin, "launchctl"),
        [
            format!("bootout gui/{uid}/io.github.michaelrigart.tagteam"),
            format!("bootstrap gui/{uid} {}", path.display()),
        ]
    );
}

#[test]
fn without_a_service_manager_install_fails_naming_print() {
    let d = tempfile::tempdir().unwrap();
    bin(d.path());
    let out = service_cmd(d.path(), "linux", &["--json", "daemon", "install"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        json_of(&out),
        json!({"schemaVersion": 1, "error": {"type": "no-service-manager",
               "message": "systemctl was not found; `tagteam daemon install --print` prints the unit to install yourself"}})
    );
    assert!(!d.path().join("home/.config/systemd").exists());
}

#[test]
fn status_without_a_daemon_exits_3_with_the_service_s_state() {
    // D8.2, Decision 6.
    let d = tempfile::tempdir().unwrap();
    let bin = bin(d.path());
    let unit = d.path().join("home/.config/systemd/user/tagteam.service");
    let out = service_cmd(d.path(), "linux", &["--json", "daemon", "status"]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
    assert_eq!(
        json_of(&out),
        json!({"schemaVersion": 1, "running": false, "pid": null, "daemon": null,
               "service": {"kind": "systemd", "path": unit.display().to_string(),
                           "installed": false, "loaded": null, "lastExitStatus": null}})
    );
    let out = service_cmd(d.path(), "linux", &["daemon", "status"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(
        text(&out.stdout),
        format!(
            "The daemon is not running.\nService: the systemd user unit at {} is not installed; its state cannot be read (systemctl is not on PATH).\n",
            unit.display()
        )
    );
    launchctl(&bin);
    let agent = d
        .path()
        .join("home/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist");
    let out = service_cmd(d.path(), "macos", &["--json", "daemon", "status"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(
        json_of(&out)["service"],
        json!({"kind": "launchd", "path": agent.display().to_string(),
               "installed": false, "loaded": false, "lastExitStatus": null})
    );
    assert_eq!(
        calls(&bin, "launchctl"),
        [format!(
            "print gui/{}/io.github.michaelrigart.tagteam",
            current_uid()
        )]
    );
}

#[test]
fn a_live_process_holding_the_daemon_lock_without_a_socket_is_running() {
    // D8.2: running, with the pid from the lock's record, and no status to show.
    let d = tempfile::tempdir().unwrap();
    bin(d.path());
    let me = ProcessStamp::current().unwrap();
    let lock = Env::for_test(d.path()).data_dir().join("locks/daemon.lock");
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let record = |start: u64| {
        format!(
            "{{\"pid\":{},\"start\":{start},\"version\":\"{}\"}}\n",
            me.pid,
            env!("CARGO_PKG_VERSION")
        )
    };
    fs::write(&lock, record(me.start)).unwrap();
    let out = service_cmd(d.path(), "linux", &["--json", "daemon", "status"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v = json_of(&out);
    assert_eq!(
        (&v["running"], &v["pid"], &v["daemon"]),
        (&json!(true), &json!(me.pid), &Value::Null)
    );
    let out = service_cmd(d.path(), "linux", &["daemon", "status"]);
    assert!(
        text(&out.stdout).starts_with(&format!(
            "A daemon runs as pid {}, but its socket does not answer; the log says why.\n",
            me.pid
        )),
        "{}",
        text(&out.stdout)
    );
    // The same record naming a process that has exited: not running.
    fs::write(&lock, record(me.start + 1)).unwrap();
    assert_eq!(
        service_cmd(d.path(), "linux", &["daemon", "status"])
            .status
            .code(),
        Some(3)
    );
}

#[test]
fn install_and_uninstall_refuse_inside_a_run_shell_and_status_answers_there() {
    // D9: a run shell's environment is a session's, not the one the daemon should keep.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    let env = Env::for_test(d.path());
    let profile = profile_path(&env, &AccountId::from_string(&a));
    ProfileMarker {
        provider: tagteam_core::ProviderId::new("claude-code"),
        account_id: AccountId::from_string(&a),
        config_dir: profile.display().to_string(),
        outer: json!({"CLAUDE_CONFIG_DIR": null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": null}),
    }
    .write(&profile)
    .unwrap();
    let bin = bin(d.path());
    launchctl(&bin);
    let inside = |args: &[&str]| {
        std_cmd(d.path())
            .env("PATH", &bin)
            .env("CLAUDE_CONFIG_DIR", &profile)
            .args(args)
            .output()
            .unwrap()
    };
    for args in [
        &["--json", "daemon", "install"][..],
        &["--json", "daemon", "install", "--print"],
        &["--json", "daemon", "uninstall"],
    ] {
        let out = inside(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(
            json_of(&out)["error"]["type"],
            json!("inside-run-shell"),
            "{args:?}"
        );
    }
    assert_eq!(
        calls(&bin, "launchctl"),
        Vec::<String>::new(),
        "a refusal runs nothing"
    );
    let out = inside(&["--json", "daemon", "status"]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["running"], json!(false));
}

/// `tagteam <args>` in this user's own environment, except the variables that place tagteam's
/// store, its state and Claude Code's home, which point into `root` (on macOS its config too,
/// which no unit path depends on there). A unit installed from here records them (D9.1), so
/// the daemon it starts serves an empty store under `root`, never this user's accounts.
fn real_env_cmd(root: &Path, args: &[&str]) -> Output {
    let mut c = std::process::Command::new(assert_cmd::cargo::cargo_bin("tagteam"));
    c.env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("CLAUDE_CONFIG_DIR", root.join("claude"))
        .env_remove("CLAUDE_SECURESTORAGE_CONFIG_DIR")
        .args(args)
        .stdin(Stdio::null());
    if cfg!(target_os = "macos") {
        c.env("XDG_CONFIG_HOME", root.join("config"));
    }
    c.output().unwrap()
}

/// Uninstalls the real unit when dropped, so a failed assertion after the install never leaves a
/// KeepAlive agent loaded. A second uninstall after a successful one removes nothing.
struct UninstallOnDrop<'a>(&'a Path);

impl Drop for UninstallOnDrop<'_> {
    fn drop(&mut self) {
        real_env_cmd(self.0, &["daemon", "uninstall"]);
    }
}

#[test]
#[ignore = "loads a real launchd agent or systemd user unit under tagteam's own label: run by hand, with Michael's go-ahead (plan B Task 10)"]
fn a_real_service_manager_starts_and_stops_the_daemon() {
    // D14: the real launchd or systemd round trip, on the platform this runs on. The unit goes
    // where the manager reads it, under the real label, so the test stops at once when a
    // tagteam unit is installed already. The unit records a fixture's store, state and Claude
    // Code home, so the daemon it starts has no account to touch.
    let d = short_tempdir();
    let root = d.path();
    let status = || {
        let out = real_env_cmd(root, &["--json", "daemon", "status"]);
        (out.status.code(), json_of(&out))
    };
    let (code, before) = status();
    // A launchd agent can stay loaded after its plist is deleted, and the install's bootout
    // would then unload the person's own agent: refuse unless the manager says, explicitly,
    // that nothing is loaded. `null` is the manager not answering, which proves nothing.
    assert_eq!(
        before["service"]["loaded"],
        json!(false),
        "a tagteam service is loaded already, or the manager did not say it is not (this test \
         would replace it): run `tagteam daemon uninstall` first, then run this test again: \
         {before}"
    );
    assert_eq!(code, Some(3), "{before}");
    assert_eq!(
        before["service"]["installed"],
        json!(false),
        "a tagteam unit is installed already; `tagteam daemon uninstall` it first: {before}"
    );
    let unit = PathBuf::from(before["service"]["path"].as_str().unwrap());

    let out = real_env_cmd(root, &["--json", "daemon", "install"]);
    let _uninstall = UninstallOnDrop(root);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["started"], json!(true));
    assert_eq!(mode(&unit), 0o600);
    let deadline = Instant::now() + Duration::from_secs(30);
    let running = loop {
        let (code, v) = status();
        if code == Some(0) && v["daemon"].is_object() {
            break v;
        }
        assert!(
            Instant::now() < deadline,
            "the service never started the daemon: {v}"
        );
        thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(running["service"]["loaded"], json!(true), "{running}");
    assert_eq!(
        running["daemon"]["daemon"]["socket"],
        json!(root.join("data/tagteam/daemon.sock").display().to_string()),
        "the daemon serves the fixture's store"
    );

    let out = real_env_cmd(root, &["--json", "daemon", "uninstall"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(json_of(&out)["removed"], json!(true));
    assert!(!unit.exists());
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (code, v) = status();
        if code == Some(3) {
            assert_eq!(v["service"]["installed"], json!(false), "{v}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the daemon outlived its unit: {v}"
        );
        thread::sleep(Duration::from_millis(200));
    }
}
