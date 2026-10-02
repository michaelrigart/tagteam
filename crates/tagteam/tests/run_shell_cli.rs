//! §12.8 through the binary: a run shell's refusals, and an unreadable marker's (Review Focus
//! 4). Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{cmd, two_fresh_accounts};
use serde_json::{Value, json};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_provider::Env;
use tagteam_provider::profile::{MARKER_FILE, ProfileMarker, canonical_profile_path};

/// Every command but `statusline`, each with arguments it accepts.
const COMMANDS: &[&[&str]] = &[
    &["list"],
    &["status"],
    &["switch"],
    &["switch", "1"],
    &["switch", "1", "--force"],
    &["add"],
    &["add-token", "sk-ant-api03-x"],
    &["remove", "1"],
    &["disable", "1"],
    &["enable", "1"],
    &["alias"],
    &["alias", "1", "work"],
    &["move", "1", "2"],
    &["history"],
    &["auto", "--once"],
    &["auto", "--once", "--dry-run"],
];

/// `id`'s profile directory under the fixture's `sessions/`.
fn profile_dir(root: &Path, id: &str) -> PathBuf {
    Env::for_test(root).data_dir().join("sessions").join(id)
}

/// `id`'s profile with a valid marker, launched from the fixture's home, where neither Claude
/// Code variable is defined (§12.2). The fixture's paths are ASCII, so the canonical path is
/// its own NFC spelling.
fn profile(root: &Path, id: &str) -> PathBuf {
    let dir = profile_dir(root, id);
    fs::create_dir_all(&dir).unwrap();
    ProfileMarker {
        provider: ProviderId::new(CLAUDE_CODE),
        account_id: AccountId::from_string(id),
        config_dir: canonical_profile_path(&dir).unwrap().display().to_string(),
        outer: json!({"CLAUDE_CONFIG_DIR": null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": null}),
    }
    .write(&dir)
    .unwrap();
    dir
}

fn json_of(out: &std::process::Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn an_unreadable_marker_refuses_every_command_but_statusline() {
    // §12.8: every refusal names the file, and the status bar shows nothing.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    let dir = profile_dir(d.path(), &a);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(MARKER_FILE), "{\"format\": \"tagteam-profile\"").unwrap();
    let marker = dir.join(MARKER_FILE).display().to_string();
    for args in COMMANDS {
        let out = cmd(d.path())
            .env("CLAUDE_CONFIG_DIR", &dir)
            .args(*args)
            .output()
            .unwrap();
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(stderr.contains(&marker), "{args:?}: {stderr}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let out = cmd(d.path())
            .env("CLAUDE_CONFIG_DIR", &dir)
            .args(*args)
            .arg("--json")
            .output()
            .unwrap();
        let v = json_of(&out);
        assert_eq!(v["error"]["type"], "run-shell-unreadable", "{args:?}");
        assert!(
            v["error"]["message"].as_str().unwrap().contains(&marker),
            "{args:?}: {v}"
        );
    }
    cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &dir)
        .arg("statusline")
        .assert()
        .success()
        .stdout("")
        .stderr("");
}

#[test]
fn a_run_shell_sees_the_default_home_and_refuses_account_changes() {
    // §12.8: the profile has no `.claude.json`, so a CLI that took it for the default home
    // would find no live login at all.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    let dir = profile(d.path(), &a);
    let out = cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &dir)
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        json_of(&out)["activeAccountNumber"],
        2,
        "b, the default login"
    );
    let out = cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &dir)
        .args(["status", "--json"])
        .output()
        .unwrap();
    assert_eq!(json_of(&out)["active"]["email"], "b@x.co");
    for args in [
        &["switch", "1"][..],
        &["add"],
        &["remove", "1"],
        &["move", "1", "2"],
    ] {
        let out = cmd(d.path())
            .env("CLAUDE_CONFIG_DIR", &dir)
            .args(args)
            .arg("--json")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(
            json_of(&out)["error"]["type"],
            "inside-run-shell",
            "{args:?}"
        );
    }
}
