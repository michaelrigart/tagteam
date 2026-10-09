//! `tagteam doctor` through the binary (§13.6): its shapes, its exit codes, and B.67 — it
//! writes nothing, creates nothing and asks nothing, over SSH-like conditions included
//! (Review Focus 4).
#![cfg(feature = "test-support")]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::{Duration, Instant};

use common::*;
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::{Env, FileKeychain, Keychain, ProfileMarker, profile_path};

/// `doctor` in the fixture, with `args`, stdin closed and no `claude` on `PATH`: the test
/// runner's own `PATH` may hold the real one, which no test runs (§15.1).
fn doctor(root: &Path, args: &[&str]) -> Output {
    std_cmd(root)
        .env("PATH", root.join("no-bin"))
        .arg("doctor")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn report(root: &Path) -> (Value, i32) {
    let out = doctor(root, &["--json"]);
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)));
    (v, out.status.code().unwrap())
}

/// The checks with `id`.
fn with_id<'v>(v: &'v Value, id: &str) -> Vec<&'v Value> {
    v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["id"] == id)
        .collect()
}

fn status_of(v: &Value, id: &str) -> String {
    let all = with_id(v, id);
    assert_eq!(all.len(), 1, "{id}: {v:#}");
    all[0]["status"].as_str().unwrap().to_owned()
}

/// Every entry under `root` with its mode and a file's bytes; a `-shm` file without them
/// (Decision 3: it is SQLite's shared-memory index, which every reader updates).
fn tree(root: &Path) -> BTreeMap<PathBuf, (u32, Option<Vec<u8>>)> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>) {
        let Ok(listing) = fs::read_dir(dir) else {
            return;
        };
        for entry in listing {
            let path = entry.unwrap().path();
            let m = fs::symlink_metadata(&path).unwrap();
            let mode = m.permissions().mode();
            if m.file_type().is_symlink() {
                let t = fs::read_link(&path).unwrap();
                out.insert(path, (mode, Some(t.into_os_string().into_encoded_bytes())));
            } else if m.is_dir() {
                out.insert(path.clone(), (mode, None));
                walk(&path, out);
            } else if path.to_string_lossy().ends_with("-shm") {
                out.insert(path, (mode, None));
            } else {
                out.insert(path.clone(), (mode, Some(fs::read(&path).unwrap())));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out);
    out
}

/// The paths `after` adds, drops or changes against `before`.
fn changed(
    before: &BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>,
    after: &BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>,
) -> Vec<PathBuf> {
    let paths: std::collections::BTreeSet<&PathBuf> = before.keys().chain(after.keys()).collect();
    paths
        .into_iter()
        .filter(|p| before.get(*p) != after.get(*p))
        .cloned()
        .collect()
}

#[test]
fn over_ssh_with_no_store_and_no_claude_doctor_answers_at_once_and_creates_nothing() {
    // Review Focus 4: the login keychain locked (the file keychain's lock flag), no store, and
    // no `claude` on PATH.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("keychain")).unwrap();
    fs::write(d.path().join("keychain/LOCKED"), "").unwrap();
    let before = tree(d.path());
    let started = Instant::now();
    let out = doctor(d.path(), &["--json"]);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(changed(&before, &tree(d.path())), Vec::<PathBuf>::new());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("Unlock it now?"),
        "doctor asks nothing: {stderr}"
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status_of(&v, "store.present"), "info");
    assert_eq!(status_of(&v, "cc.keychain"), "warn");
    assert_eq!(status_of(&v, "cc.binary"), "warn");
    assert_eq!(out.status.code(), Some(0), "nothing fails: {v:#}");

    let human = doctor(d.path(), &[]);
    assert_eq!(
        changed(&before, &tree(d.path())),
        Vec::<PathBuf>::new(),
        "the human form writes nothing either"
    );
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(text.contains("· tagteam has no state yet"), "{text}");
    assert!(text.contains("! the login keychain is locked"), "{text}");
    assert!(
        text.contains("    fix: `security unlock-keychain ~/Library/Keychains/login.keychain-db`"),
        "{text}"
    );
    assert!(text.lines().last().unwrap().ends_with(" fail"), "{text}");
}

#[test]
fn the_json_shape_of_a_fixture_with_two_accounts() {
    let d = tempfile::tempdir().unwrap();
    two_fresh_accounts(d.path());
    let (v, code) = report(d.path());
    assert_eq!(code, 0);
    let top: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(top, ["schemaVersion", "ok", "checks"]);
    assert_eq!(v["schemaVersion"], json!(1));
    assert_eq!(v["ok"], json!(true));
    let root = d.path().display().to_string();
    let shape: Vec<Value> = v["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let keys: Vec<&str> = c.as_object().unwrap().keys().map(String::as_str).collect();
            assert_eq!(keys, ["id", "provider", "status", "message", "fix"], "{c}");
            assert!(!c["message"].as_str().unwrap().is_empty(), "{c}");
            json!({
                "id": c["id"],
                "provider": c["provider"],
                "status": c["status"],
                "fix": c["fix"].as_str().map(|f| f.replace(&root, "<root>")),
            })
        })
        .collect();
    let own =
        |id: &str, status: &str| json!({"id": id, "provider": null, "status": status, "fix": null});
    let cc = |id: &str, status: &str| json!({"id": id, "provider": "claude-code", "status": status, "fix": null});
    assert_eq!(
        shape,
        vec![
            own("store.integrity", "ok"),
            own("store.schema", "ok"),
            own("store.mode", "ok"),
            own("settings.file", "ok"),
            own("log.file", "info"),
            own("log.writable", "ok"),
            own("store.temp-files", "ok"),
            cc("accounts.vault", "ok"),
            cc("usage.state", "ok"),
            cc("switch.interrupted", "ok"),
            cc("auto.engine", "ok"),
            cc("sessions.unknown-entries", "ok"),
            json!({"id": "cc.binary", "provider": "claude-code", "status": "warn",
                "fix": "install Claude Code, or add its directory to PATH"}),
            cc("cc.paths", "info"),
            cc("cc.keychain", "ok"),
            cc("cc.keychain-account", "ok"),
            cc("cc.env", "ok"),
            cc("cc.locks", "ok"),
        ]
    );
}

#[test]
fn doctor_exits_1_when_a_check_fails_and_names_the_fix_beneath_it() {
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    FileKeychain::new(d.path().join("keychain"))
        .delete(SERVICE, &a)
        .unwrap();
    let (v, code) = report(d.path());
    assert_eq!(code, 1);
    assert_eq!(v["ok"], json!(false));
    assert_eq!(status_of(&v, "accounts.vault"), "fail");
    let out = doctor(d.path(), &[]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("✗ account 1 has no vault entry")
            && text.contains("    fix: `tagteam remove 1` finishes deleting it"),
        "{text}"
    );
}

#[test]
fn doctor_over_a_full_fixture_home_leaves_every_byte_as_it_was() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = two_fresh_accounts(d.path());
    let env = Env::for_test(d.path());
    let data = env.data_dir();
    // A profile, a pending rescue, a displaced file, an engine record, a temp file, settings.
    let profile = profile_path(&env, &AccountId::from_string(&b));
    ProfileMarker {
        provider: tagteam_core::ProviderId::new("claude-code"),
        account_id: AccountId::from_string(&b),
        config_dir: profile.display().to_string(),
        outer: json!({"CLAUDE_CONFIG_DIR": null, "CLAUDE_SECURESTORAGE_CONFIG_DIR": null}),
    }
    .write(&profile)
    .unwrap();
    fs::create_dir_all(data.join("rescue")).unwrap();
    fs::write(data.join(format!("rescue/{a}-0-0123456789ab.json")), "{}").unwrap();
    fs::create_dir_all(data.join("displaced")).unwrap();
    fs::write(
        data.join("displaced/1790000000-0123456789ab-aaaaaa.json"),
        "x",
    )
    .unwrap();
    fs::create_dir_all(data.join("locks")).unwrap();
    fs::write(
        data.join("locks/autoswitch-claude-code.lock"),
        "{\"pid\":1,\"start\":1}\n",
    )
    .unwrap();
    fs::write(data.join(".tagteam.db.tagteam-4242-0a1b2c3d"), "s").unwrap();
    fs::create_dir_all(env.config_dir()).unwrap();
    fs::write(env.config_dir().join("config.toml"), "bogus = 1\n").unwrap();
    let before = tree(d.path());
    for args in [&[][..], &["--json"], &["--json", "--online"]] {
        doctor(d.path(), args);
        assert_eq!(
            changed(&before, &tree(d.path())),
            Vec::<PathBuf>::new(),
            "{args:?}"
        );
    }
}

#[test]
fn doctor_runs_inside_a_run_shell_and_refuses_under_an_unreadable_marker() {
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
    let inside = std_cmd(d.path())
        .env("PATH", d.path().join("no-bin"))
        .env("CLAUDE_CONFIG_DIR", &profile)
        .args(["doctor", "--json"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        inside.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&inside.stdout)
    );
    let v: Value = serde_json::from_slice(&inside.stdout).unwrap();
    let paths = &with_id(&v, "cc.paths")[0]["message"];
    assert!(
        paths.as_str().unwrap().contains("/home/.claude,"),
        "the outer home's paths (§12.8): {paths}"
    );

    fs::write(profile.join(".tagteam-profile.json"), "not json").unwrap();
    let refused = std_cmd(d.path())
        .env("PATH", d.path().join("no-bin"))
        .env("CLAUDE_CONFIG_DIR", &profile)
        .args(["doctor", "--json"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&refused.stdout).unwrap();
    assert_eq!(v["error"]["type"], json!("run-shell-unreadable"));
}
