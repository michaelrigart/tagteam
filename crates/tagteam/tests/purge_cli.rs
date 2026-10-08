//! §10.5 through the binary: `tagteam purge`'s JSON and words, its confirmation rule, what it
//! keeps, and its refusals. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::{LOCKED, cc_profile, cmd, live_email, two_fresh_accounts};
use serde_json::{Value, json};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::{Env, FileKeychain, Keychain, Read};

fn json_of(out: &std::process::Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

/// `tagteam purge <args>`'s output.
fn purge(root: &Path, args: &[&str]) -> std::process::Output {
    cmd(root).arg("purge").args(args).output().unwrap()
}

fn keychain(root: &Path) -> FileKeychain {
    FileKeychain::new(root.join("keychain"))
}

#[test]
fn a_full_purge_reports_in_json_and_keeps_the_settings_and_the_live_login() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = two_fresh_accounts(d.path());
    let env = Env::for_test(d.path());
    let config = env.config_dir().join("config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "[autoswitch]\nthreshold = 80\n").unwrap();
    let out = purge(d.path(), &["--yes", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        json_of(&out),
        json!({"schemaVersion": 1, "ok": true, "provider": null,
               "accounts": [{"number": 1, "id": a, "email": "a@x.co"},
                            {"number": 2, "id": b, "email": "b@x.co"}],
               "displaced": 0, "rescues": 0, "storeEmptied": true, "failures": []})
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "[autoswitch]\nthreshold = 80\n"
    );
    assert!(!env.log_file().exists());
    assert_eq!(live_email(d.path()), "b@x.co", "B.66: the live login stays");
    assert!(matches!(
        keychain(d.path()).service_has_items(SERVICE),
        Read::Present(false)
    ));
    // The store is still there, empty.
    let list = cmd(d.path()).args(["list", "--json"]).output().unwrap();
    assert_eq!(json_of(&list)["accounts"], json!([]));
}

#[test]
fn the_result_in_words_names_each_account() {
    let d = tempfile::tempdir().unwrap();
    two_fresh_accounts(d.path());
    cmd(d.path())
        .args(["purge", "--yes"])
        .assert()
        .success()
        .stdout(
            "Purged a@x.co (position 1).\nPurged b@x.co (position 2).\nEmptied the store and deleted the log.\n",
        )
        .stderr("");
}

#[test]
fn without_yes_and_without_a_terminal_it_needs_confirmation_and_deletes_nothing() {
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    let out = purge(d.path(), &["--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        json_of(&out),
        json!({"schemaVersion": 1, "error": {"type": "needs-confirmation",
               "message": "purge deletes tagteam's data for good; run it on a terminal to confirm, or pass --yes"}})
    );
    let out = purge(d.path(), &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert!(keychain(d.path()).find(SERVICE, &a).is_present());
}

#[test]
fn a_provider_purge_leaves_the_store_and_the_log() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = two_fresh_accounts(d.path());
    let out = purge(d.path(), &["--provider", "claude-code", "--yes", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json_of(&out);
    assert_eq!(
        (
            v["provider"].clone(),
            v["storeEmptied"].clone(),
            v["accounts"].clone()
        ),
        (
            json!("claude-code"),
            json!(false),
            json!([{"number": 1, "id": a, "email": "a@x.co"},
                   {"number": 2, "id": b, "email": "b@x.co"}])
        )
    );
}

#[test]
fn keychain_orphans_with_a_provider_is_a_usage_error() {
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    let out = purge(
        d.path(),
        &[
            "--provider",
            "claude-code",
            "--keychain-orphans",
            "--yes",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json_of(&out)["error"]["type"], "usage");
    assert!(keychain(d.path()).find(SERVICE, &a).is_present());
}

#[test]
fn another_data_directory_s_keychain_items_stay_unless_keychain_orphans_is_given() {
    // §10.5: every tagteam data directory on a Mac shares the `tagteam` service.
    let d = tempfile::tempdir().unwrap();
    two_fresh_accounts(d.path());
    let kc = keychain(d.path());
    kc.upsert(SERVICE, "another-data-directory-s-account", b"theirs")
        .unwrap();
    let out = purge(d.path(), &["--yes"]);
    assert_eq!(out.status.code(), Some(0));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.starts_with("warning: the Keychain still holds `tagteam` items"),
        "{stderr}"
    );
    assert!(stderr.contains("--keychain-orphans"), "{stderr}");
    assert!(
        kc.find(SERVICE, "another-data-directory-s-account")
            .is_present()
    );
    let out = purge(d.path(), &["--keychain-orphans", "--yes"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert!(matches!(
        kc.service_has_items(SERVICE),
        Read::Present(false)
    ));
}

#[test]
fn inside_a_run_shell_purge_refuses_before_the_keychain_check() {
    // §10.5: "Inside a run shell, purge refuses at once", before step 1's lock check.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    let (_, shell) = cc_profile(d.path(), &a);
    fs::write(d.path().join("keychain/LOCKED"), "").unwrap();
    let out = cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &shell)
        .args(["purge", "--yes", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json_of(&out)["error"]["type"], "inside-run-shell");
    fs::remove_file(d.path().join("keychain/LOCKED")).unwrap();
    assert!(keychain(d.path()).find(SERVICE, &a).is_present());
}

#[test]
fn a_locked_keychain_refuses_before_anything_is_deleted() {
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    fs::write(d.path().join("keychain/LOCKED"), "").unwrap();
    cmd(d.path())
        .args(["purge", "--yes"])
        .assert()
        .code(1)
        .stderr(format!("tagteam: {LOCKED}\n"));
    fs::remove_file(d.path().join("keychain/LOCKED")).unwrap();
    assert!(keychain(d.path()).find(SERVICE, &a).is_present());
}

#[test]
fn what_could_not_be_deleted_is_reported_and_exits_1() {
    // §10.5 "Result": the purge goes on, reports the failure and exits 1.
    let d = tempfile::tempdir().unwrap();
    two_fresh_accounts(d.path());
    let state = Env::for_test(d.path()).state_dir();
    fs::create_dir_all(&state).unwrap();
    let log = state.join("tagteam.log");
    fs::write(&log, "line\n").unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o500)).unwrap();
    let out = purge(d.path(), &["--yes", "--json"]);
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = json_of(&out);
    assert_eq!(
        (v["ok"].clone(), v["storeEmptied"].clone()),
        (json!(false), json!(true))
    );
    assert_eq!(v["accounts"].as_array().unwrap().len(), 2);
    let failures = v["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 1, "{v}");
    assert_eq!(failures[0]["what"], log.display().to_string());
    assert!(log.exists());
    // In words, the failure goes to stderr and the result to stdout.
    fs::set_permissions(&state, fs::Permissions::from_mode(0o500)).unwrap();
    let out = purge(d.path(), &["--yes"]);
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.starts_with(&format!("tagteam: could not delete {}: ", log.display())),
        "{stderr}"
    );
}

#[test]
fn an_account_that_cannot_be_deleted_keeps_the_store_and_the_next_purge_finishes_it() {
    // §10.5: "running purge or remove again finishes it". No vault entry can be deleted, so
    // the purge exits 1 and keeps the store, whose rows the next purge finishes from.
    let d = tempfile::tempdir().unwrap();
    let (a, b) = two_fresh_accounts(d.path());
    let items = d.path().join("keychain");
    fs::set_permissions(&items, fs::Permissions::from_mode(0o500)).unwrap();
    let out = purge(d.path(), &["--yes", "--json"]);
    fs::set_permissions(&items, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = json_of(&out);
    assert_eq!(
        (
            v["ok"].clone(),
            v["storeEmptied"].clone(),
            v["accounts"].clone()
        ),
        (json!(false), json!(false), json!([])),
        "{v}"
    );
    let what: Vec<&str> = v["failures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["what"].as_str().unwrap())
        .collect();
    assert_eq!(
        what,
        [
            "claude-code #1 (a@x.co)",
            "claude-code #2 (b@x.co)",
            "the store"
        ],
        "{v}"
    );
    let out = purge(d.path(), &["--yes", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json_of(&out);
    assert_eq!(
        (v["storeEmptied"].clone(), v["accounts"].clone()),
        (
            json!(true),
            json!([{"number": 1, "id": a, "email": "a@x.co"},
                   {"number": 2, "id": b, "email": "b@x.co"}])
        )
    );
    assert!(matches!(
        keychain(d.path()).service_has_items(SERVICE),
        Read::Present(false)
    ));
}

#[test]
fn a_purge_that_deleted_nothing_in_words_does_not_say_there_was_nothing() {
    // §10.5: it reports what it could not delete; the failures are all it prints.
    let d = tempfile::tempdir().unwrap();
    two_fresh_accounts(d.path());
    let items = d.path().join("keychain");
    fs::set_permissions(&items, fs::Permissions::from_mode(0o500)).unwrap();
    let out = purge(d.path(), &["--yes"]);
    fs::set_permissions(&items, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("tagteam: could not delete claude-code #1 (a@x.co): "),
        "{stderr}"
    );
}

#[test]
fn keychain_orphans_with_a_provider_in_words_names_the_conflict() {
    let d = tempfile::tempdir().unwrap();
    two_fresh_accounts(d.path());
    let out = purge(
        d.path(),
        &["--provider", "claude-code", "--keychain-orphans", "--yes"],
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("--keychain-orphans"), "{stderr}");
    assert!(stderr.contains("--provider"), "{stderr}");
}

#[test]
fn the_usage_error_comes_before_the_run_shell_refusal() {
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    let (_, shell) = cc_profile(d.path(), &a);
    let out = cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &shell)
        .args([
            "purge",
            "--provider",
            "claude-code",
            "--keychain-orphans",
            "--yes",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json_of(&out)["error"]["type"], "usage");
    assert!(keychain(d.path()).find(SERVICE, &a).is_present());
}
