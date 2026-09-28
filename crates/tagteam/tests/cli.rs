//! Drives the real binary. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::path::Path;

use assert_cmd::Command;
use common::LOCKED;
use serde_json::{Value, json};

fn cmd(root: &Path) -> Command {
    let mut c = Command::cargo_bin("tagteam").unwrap();
    c.env_clear()
        .env("HOME", root.join("home"))
        .env("USER", "tester")
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("TAGTEAM_TEST_KEYCHAIN_DIR", root.join("keychain"))
        .env("TAGTEAM_TEST_PLATFORM", "macos");
    c
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
