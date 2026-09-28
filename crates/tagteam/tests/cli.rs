//! Drives the real binary. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::os::fd::{FromRawFd, OwnedFd};
use std::path::Path;
use std::process::Stdio;

use assert_cmd::assert::OutputAssertExt;
use common::{LOCKED, cmd, login, seed_home, std_cmd};
use predicates::prelude::PredicateBooleanExt;
use serde_json::{Value, json};
use tagteam_provider::{Env, FileKeychain};

/// A stdout whose reader is already gone, so every write to it fails with EPIPE.
fn closed_stdout() -> Stdio {
    let mut fds = [0; 2];
    // SAFETY: `pipe` fills `fds` with two new descriptors on success, and each is owned once.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    drop(read);
    Stdio::from(write)
}

fn assert_home_empty(root: &Path) {
    assert!(
        std::fs::read_dir(root.join("home"))
            .unwrap()
            .next()
            .is_none(),
        "HOME must stay empty"
    );
}

#[test]
fn a_fresh_machine_lists_nothing_and_creates_nothing() {
    // Review Focus 4: no `~/.claude`, no `~/.claude.json`, and nothing created by any of this.
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    let out = cmd(d.path())
        .args(["list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "activeAccountNumber": null, "activeByProvider": {"claude-code": null}, "accounts": []})
    );
    cmd(d.path())
        .args(["status", "--json"])
        .assert()
        .success()
        .stdout("{\"schemaVersion\":1,\"provider\":\"claude-code\",\"active\":null}\n");
    assert_home_empty(d.path());
    cmd(d.path())
        .arg("add")
        .assert()
        .code(1)
        .stderr(predicates::str::contains("no live login"));
    assert_home_empty(d.path());
}

#[test]
fn usage_errors_exit_2_and_keep_the_json_contract() {
    let d = tempfile::tempdir().unwrap();
    cmd(d.path()).args(["move", "1"]).assert().code(2);
    cmd(d.path()).arg("frobnicate").assert().code(2);
    cmd(d.path()).arg("--bogus").assert().code(2);
    let out = cmd(d.path())
        .args(["frobnicate", "--json"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap()["error"]["type"],
        "usage"
    );
}

#[test]
fn a_usage_error_never_echoes_an_argument() {
    // Secrets never reach error output, and a token is an argument like any other, whatever
    // its shape: no usage error repeats an argument's value.
    const SENTINEL: &str = "REVIEW-SENTINEL";
    let d = tempfile::tempdir().unwrap();
    let cases: [&[&str]; 6] = [
        &["add-token", "-", "sk-ant-api03-REVIEW-SENTINEL"],
        &["add-token", "-", "plain-REVIEW-SENTINEL"],
        &["add-token", "tok", "--position", "REVIEW-SENTINEL"],
        &["add-token", "--REVIEW-SENTINEL"],
        &["REVIEW-SENTINEL"],
        &["add-token", "-", "sk-ant-api03-REVIEW-SENTINEL", "--json"],
    ];
    for args in cases {
        let out = cmd(d.path())
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .clone();
        let (stdout, stderr) = (
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(
            !stdout.contains(SENTINEL) && !stderr.contains(SENTINEL),
            "{args:?}:\n{stdout}\n{stderr}"
        );
        if !args.contains(&"--json") {
            assert!(stderr.starts_with("error: "), "{args:?}: {stderr}");
        }
    }
    // The kind and the usage line, which come from the command's own definition, stay.
    cmd(d.path())
        .args(["add-token", "-", "sk-ant-api03-REVIEW-SENTINEL"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(
            "error: unexpected argument found\n\nUsage: tagteam add-token [OPTIONS] [TOKEN]\n\nFor more information, try '--help'.\n",
        );
    // So does a suggestion: it names a real name, never what was typed.
    cmd(d.path())
        .arg("swtich")
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "a similar subcommand exists: 'switch'",
        ));
}

#[test]
fn help_and_version_under_json_are_a_json_usage_error() {
    // They print text, and `--json` promises exactly one JSON object on stdout.
    let d = tempfile::tempdir().unwrap();
    for flag in ["--help", "--version"] {
        let out = cmd(d.path())
            .args([flag, "--json"])
            .assert()
            .code(2)
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json!({"schemaVersion": 1, "error": {"type": "usage",
                   "message": "--help and --version print text; run them without --json"}}),
            "{flag}"
        );
    }
    // Without `--json`, help is the usual page on stdout.
    cmd(d.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::starts_with(
            "Multi-account switcher for AI coding agent CLIs\n",
        ));
}

#[test]
fn a_closed_stdout_is_not_a_panic() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    let cases: [(&[&str], i32); 3] = [
        (&["frobnicate", "--json"], 2),
        (&["list", "--json"], 0),
        (&["list"], 0),
    ];
    for (args, code) in cases {
        std_cmd(d.path())
            .args(args)
            .stdout(closed_stdout())
            .output()
            .unwrap()
            .assert()
            .code(code)
            .stderr(predicates::str::contains("panicked").not());
    }
}

#[test]
fn add_token_reads_a_line_from_stdin() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    cmd(d.path())
        .args(["add-token", "-"])
        .write_stdin("sk-ant-api03-from-stdin\n")
        .assert()
        .success();
    let out = cmd(d.path())
        .args(["list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["accounts"][0]["usageStatus"], "api_key");
}

#[test]
fn a_locked_keychain_without_a_terminal_fails_and_creates_nothing() {
    // Appendix A.3 through the binary: no terminal, so no prompt; `list` and `status` run no check.
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    std::fs::create_dir_all(d.path().join("keychain")).unwrap();
    std::fs::write(d.path().join("keychain/LOCKED"), "").unwrap();
    cmd(d.path()).args(["list", "--json"]).assert().success();
    cmd(d.path()).args(["status", "--json"]).assert().success();
    cmd(d.path())
        .arg("add")
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!("tagteam: {LOCKED}\n"));
    let out = cmd(d.path())
        .args(["add-token", "sk-ant-api03-key", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "keychain-locked", "message": LOCKED}})
    );
    assert_home_empty(d.path());
    // Linux has no Keychain, so no check.
    cmd(d.path())
        .args(["add-token", "sk-ant-api03-key"])
        .env("TAGTEAM_TEST_PLATFORM", "linux")
        .assert()
        .success();
}

#[test]
fn a_routine_switch_is_quiet_and_debug_shows_the_diagnostics() {
    // Claude Code rotated the live credential, and M1 has no oracle to attribute it, so every
    // such switch captures it with a WARN log line (§9.4 step 4). That is a diagnostic: the
    // default level keeps it off stderr, and `--debug` shows it.
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = FileKeychain::new(d.path().join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b-rotated");
    let out = cmd(d.path())
        .args(["switch", "1", "--json"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    // Exactly one object: anything after it would fail to parse.
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        (v["switched"].clone(), v["to"].clone()),
        (json!(true), json!(1))
    );
    login(&env, &kc, "a@x.co", "", "rt-a-rotated");
    let out = cmd(d.path())
        .args(["switch", "2", "--json", "--debug"])
        .assert()
        .success()
        .stderr(predicates::str::contains(
            "captured an unverified live credential",
        ))
        .get_output()
        .stdout
        .clone();
    assert_eq!(serde_json::from_slice::<Value>(&out).unwrap()["to"], 2);
}
