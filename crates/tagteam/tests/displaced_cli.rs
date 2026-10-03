//! `tagteam displaced` through the binary (§6.3), with Review Focus 3. Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{cmd, login, seed_home};
use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_provider::{Env, FileKeychain};

fn displaced_dir(root: &Path) -> PathBuf {
    Env::for_test(root).data_dir().join("displaced")
}

fn listing(root: &Path) -> Value {
    let out = cmd(root)
        .args(["displaced", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).unwrap()
}

/// `a@x.co` stored at position 1, then a stranger's login that `switch 1 --force` displaced
/// (§9.4 step 2). This gives one real entry, as a user meets one. Returns its ID.
fn forced_over_a_stranger(root: &Path) -> String {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(root).arg("add").assert().success();
    login(&env, &kc, "stranger@x.co", "", "rt-s");
    cmd(root)
        .args(["switch", "1", "--force"])
        .assert()
        .success();
    listing(root)["displaced"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The second an ID carries, as the listing prints it.
fn when(id: &str) -> String {
    format_iso8601(id.split('-').next().unwrap().parse().unwrap())
}

/// The fingerprint's 12 hex digits an ID carries.
fn fp12(id: &str) -> &str {
    id.split('-').nth(1).unwrap()
}

#[test]
fn the_listing_joins_the_row_and_its_file() {
    let d = tempfile::tempdir().unwrap();
    let id = forced_over_a_stranger(d.path());
    let v = listing(d.path());
    let fingerprint = v["displaced"][0]["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        fingerprint.starts_with(&format!("sha256:{}", fp12(&id))),
        "{fingerprint}"
    );
    assert_eq!(
        v,
        json!({
            "schemaVersion": 1,
            "dir": displaced_dir(d.path()).to_str().unwrap(),
            "displaced": [{
                "id": id, "provider": "claude-code", "at": when(&id), "reason": "forced-activation",
                "fingerprint": fingerprint,
                "identity": {"emailAddress": "stranger@x.co", "organizationUuid": "",
                             "accountUuid": "uuid-stranger@x.co-"},
                "account": null, "file": "present", "recorded": true,
            }]
        })
    );
    assert!(displaced_dir(d.path()).join(format!("{id}.json")).is_file());
}

#[test]
fn the_human_listing_names_each_column_and_the_directory() {
    let d = tempfile::tempdir().unwrap();
    let id = forced_over_a_stranger(d.path());
    let out = cmd(d.path())
        .arg("displaced")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        format!(
            "ID                              WHEN                  PROVIDER     REASON             IDENTITY       ACCOUNT  FP\n\
             {id}  {}  claude-code  forced-activation  stranger@x.co  —        {}\n\
             \n\
             The files are {}/<ID>.json; tagteam never reads one back, so restoring one is manual.\n",
            when(&id),
            fp12(&id),
            displaced_dir(d.path()).display()
        )
    );
}

#[test]
fn a_fresh_machine_lists_nothing_and_creates_nothing() {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    fs::create_dir_all(&home).unwrap();
    cmd(d.path())
        .arg("displaced")
        .assert()
        .success()
        .stdout("No displaced credentials.\n");
    assert_eq!(
        listing(d.path()),
        json!({"schemaVersion": 1, "dir": displaced_dir(d.path()).to_str().unwrap(), "displaced": []})
    );
    assert!(
        fs::read_dir(&home).unwrap().next().is_none(),
        "HOME must stay empty"
    );
}
