//! §12.7: `map`, `unmap` and `shell-init`, through the real binary. Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use common::{cc_profile, cmd, two_accounts};
use serde_json::{Value, json};
use tagteam_provider::Env;

fn json_of(out: &[u8]) -> Value {
    serde_json::from_slice(out).unwrap()
}

/// `rel` under `root`, created, and its canonical spelling: what `map` stores for it.
fn dir(root: &Path, rel: &str) -> (PathBuf, String) {
    let p = root.join(rel);
    fs::create_dir_all(&p).unwrap();
    let canonical = fs::canonicalize(&p).unwrap();
    (p, canonical.into_os_string().into_string().unwrap())
}

/// `map --json`'s rows.
fn listed(root: &Path) -> Vec<Value> {
    let out = cmd(root).args(["map", "--json"]).assert().success();
    json_of(&out.get_output().stdout)["mappings"]
        .as_array()
        .unwrap()
        .clone()
}

/// `map --json`'s rows without `addedAt`, once each is checked to be ISO 8601 UTC.
fn listed_shapes(root: &Path) -> Vec<Value> {
    listed(root)
        .into_iter()
        .map(|mut row| {
            let at = row.as_object_mut().unwrap().remove("addedAt").unwrap();
            let at = at.as_str().unwrap();
            assert!(at.len() == 20 && at.ends_with('Z'), "{at}");
            row
        })
        .collect()
}

#[test]
fn map_stores_the_canonical_path_even_through_a_symlink() {
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    let (_, real) = dir(d.path(), "work/real/app");
    symlink(d.path().join("work/real"), d.path().join("work/link")).unwrap();
    let out = cmd(d.path())
        .args(["map", "1"])
        .arg(d.path().join("work/link/app"))
        .arg("--json")
        .assert()
        .success();
    let v = json_of(&out.get_output().stdout);
    assert_eq!(v["schemaVersion"], 1);
    assert_eq!(v["ok"], true);
    assert_eq!(v["mapping"]["path"], real.as_str());
    assert_eq!(v["mapping"]["id"], a.as_str());
    assert_eq!(v["mapping"]["number"], 1);
    assert_eq!(listed(d.path()).len(), 1);
}

#[test]
fn map_defaults_to_the_current_directory_and_a_new_mapping_replaces_the_old() {
    // §12.7: one mapping per provider per path.
    let d = tempfile::tempdir().unwrap();
    let (_, b) = two_accounts(d.path());
    let (app, real) = dir(d.path(), "work/app");
    cmd(d.path())
        .current_dir(&app)
        .args(["map", "1"])
        .assert()
        .success()
        .stdout(format!("Mapped {real} to a@x.co (position 1).\n"));
    cmd(d.path())
        .current_dir(&app)
        .args(["map", "b@x.co"])
        .assert()
        .success();
    assert_eq!(
        listed_shapes(d.path()),
        vec![
            json!({"path": real, "provider": "claude-code", "number": 2, "id": b, "email": "b@x.co"})
        ]
    );
}

#[test]
fn map_lists_every_mapping_in_path_order() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = two_accounts(d.path());
    let (x, x_real) = dir(d.path(), "x");
    let (y, y_real) = dir(d.path(), "y");
    cmd(d.path()).args(["map", "2"]).arg(&y).assert().success();
    cmd(d.path()).args(["map", "1"]).arg(&x).assert().success();
    assert_eq!(
        listed_shapes(d.path()),
        vec![
            json!({"path": x_real, "provider": "claude-code", "number": 1, "id": a, "email": "a@x.co"}),
            json!({"path": y_real, "provider": "claude-code", "number": 2, "id": b, "email": "b@x.co"}),
        ]
    );
    cmd(d.path())
        .arg("map")
        .assert()
        .success()
        .stdout(format!("{x_real}  1  a@x.co\n{y_real}  2  b@x.co\n"));
}

#[test]
fn unmap_removes_a_path_s_mappings_for_one_provider_or_all() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let (x, real) = dir(d.path(), "x");
    cmd(d.path()).args(["map", "1"]).arg(&x).assert().success();
    let out = cmd(d.path())
        .arg("unmap")
        .arg(&x)
        .args(["--provider", "claude-code", "--json"])
        .assert()
        .success();
    assert_eq!(
        json_of(&out.get_output().stdout),
        json!({"schemaVersion": 1, "ok": true, "path": real, "removed": 1})
    );
    assert!(listed(d.path()).is_empty());
    cmd(d.path()).args(["map", "1"]).arg(&x).assert().success();
    cmd(d.path())
        .current_dir(&x)
        .arg("unmap")
        .assert()
        .success()
        .stdout(format!("Unmapped {real}.\n"));
    cmd(d.path())
        .current_dir(&x)
        .arg("unmap")
        .assert()
        .success()
        .stdout(format!("No mapping for {real}.\n"));
}

#[test]
fn unmap_takes_the_listed_path_of_a_directory_that_is_gone() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let (gone, real) = dir(d.path(), "gone");
    cmd(d.path())
        .args(["map", "1"])
        .arg(&gone)
        .assert()
        .success();
    fs::remove_dir(&gone).unwrap();
    let out = cmd(d.path())
        .args(["unmap", &real, "--json"])
        .assert()
        .success();
    assert_eq!(json_of(&out.get_output().stdout)["removed"], 1);
    assert!(listed(d.path()).is_empty());
}

#[test]
fn map_refuses_a_path_that_is_missing_or_not_a_directory() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let file = d.path().join("file");
    fs::write(&file, "").unwrap();
    for (path, why) in [
        (d.path().join("missing"), "No such file or directory"),
        (file, "not a directory"),
    ] {
        let out = cmd(d.path())
            .args(["map", "1"])
            .arg(&path)
            .arg("--json")
            .assert()
            .code(1);
        let v = json_of(&out.get_output().stdout);
        assert_eq!(v["error"]["type"], "invalid-input", "{v}");
        let message = v["error"]["message"].as_str().unwrap();
        assert!(
            message.contains(&path.display().to_string()) && message.contains(why),
            "{message}"
        );
    }
    assert!(listed(d.path()).is_empty());
}

#[test]
fn map_needs_an_account_that_exists() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let (x, _) = dir(d.path(), "x");
    let out = cmd(d.path())
        .args(["map", "9"])
        .arg(&x)
        .arg("--json")
        .assert()
        .code(1);
    assert_eq!(
        json_of(&out.get_output().stdout)["error"]["type"],
        "no-such-account"
    );
}

#[test]
fn removing_an_account_removes_its_mappings() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let (x, _) = dir(d.path(), "x");
    let (y, y_real) = dir(d.path(), "y");
    cmd(d.path()).args(["map", "1"]).arg(&x).assert().success();
    cmd(d.path()).args(["map", "2"]).arg(&y).assert().success();
    cmd(d.path()).args(["remove", "1"]).assert().success();
    let left: Vec<Value> = listed(d.path())
        .into_iter()
        .map(|r| r["path"].clone())
        .collect();
    assert_eq!(left, [json!(y_real)]);
}

#[test]
fn on_a_fresh_machine_map_and_unmap_create_nothing() {
    // §5: a command that changes nothing creates nothing.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    let out = cmd(d.path()).args(["map", "--json"]).assert().success();
    assert_eq!(
        json_of(&out.get_output().stdout),
        json!({"schemaVersion": 1, "mappings": []})
    );
    cmd(d.path())
        .arg("map")
        .assert()
        .success()
        .stdout("No mappings yet. Map a directory with `tagteam map ACCOUNT [PATH]`.\n");
    let out = cmd(d.path())
        .args(["unmap", "/nowhere", "--json"])
        .assert()
        .success();
    assert_eq!(
        json_of(&out.get_output().stdout),
        json!({"schemaVersion": 1, "ok": true, "path": "/nowhere", "removed": 0})
    );
    assert!(!Env::for_test(d.path()).data_dir().exists());
}

#[test]
fn map_works_inside_a_run_shell_against_the_outer_home_s_store() {
    // B.32: only commands that change accounts or the live login refuse in a run shell, and
    // a run shell's commands see the outer home (§12.8).
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    let (_, spelling) = cc_profile(d.path(), &a);
    let (x, real) = dir(d.path(), "x");
    cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &spelling)
        .args(["map", "2"])
        .arg(&x)
        .assert()
        .success();
    let rows = listed(d.path());
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (&rows[0]["path"], &rows[0]["number"]),
        (&json!(real), &json!(2))
    );
}
