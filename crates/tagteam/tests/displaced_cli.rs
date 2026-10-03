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

const NEEDS_YES: &str =
    "displaced credentials cannot be recovered once deleted; pass --yes to delete them";
const YES_ALONE: &str = "--yes confirms --purge; name the entries to delete with --purge ID...";

#[test]
fn purge_with_yes_deletes_the_file_and_the_row() {
    let d = tempfile::tempdir().unwrap();
    let id = forced_over_a_stranger(d.path());
    cmd(d.path())
        .args(["displaced", "--purge", &id, "--yes"])
        .assert()
        .success()
        .stdout(format!("Deleted {id}.\n"));
    assert!(!displaced_dir(d.path()).join(format!("{id}.json")).exists());
    cmd(d.path())
        .arg("displaced")
        .assert()
        .success()
        .stdout("No displaced credentials.\n");
}

#[test]
fn without_a_terminal_or_under_json_a_purge_needs_yes() {
    let d = tempfile::tempdir().unwrap();
    let id = forced_over_a_stranger(d.path());
    let file = displaced_dir(d.path()).join(format!("{id}.json"));
    cmd(d.path())
        .args(["displaced", "--purge", &id])
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!("tagteam: {NEEDS_YES}\n"));
    let out = cmd(d.path())
        .args(["displaced", "--purge", &id, "--json"])
        .assert()
        .code(1)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "needs-confirmation", "message": NEEDS_YES}})
    );
    assert!(file.exists());
    let out = cmd(d.path())
        .args(["displaced", "--purge", &id, "--yes", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "ok": true, "deleted": [id]})
    );
    assert!(!file.exists());
}

#[test]
fn an_id_that_names_no_entry_is_refused_and_nothing_is_deleted() {
    // Review Focus 3: refused before any path is built, even beside a valid ID. A file whose
    // name differs only in case exists, and is still no entry.
    let d = tempfile::tempdir().unwrap();
    let id = forced_over_a_stranger(d.path());
    let upper = format!("{}-{}-AAAAAA", id.split('-').next().unwrap(), fp12(&id));
    fs::write(
        displaced_dir(d.path()).join(format!("{upper}.json")),
        "not an entry",
    )
    .unwrap();
    for bad in ["../../tagteam.db", "x", "", upper.as_str()] {
        for ids in [[id.as_str(), bad], [bad, id.as_str()]] {
            let out = cmd(d.path())
                .args(["displaced", "--json", "--yes", "--purge"])
                .args(ids)
                .assert()
                .code(1)
                .stderr("")
                .get_output()
                .stdout
                .clone();
            assert_eq!(
                serde_json::from_slice::<Value>(&out).unwrap(),
                json!({"schemaVersion": 1, "error": {"type": "no-such-displaced",
                       "message": format!("no displaced credential matches {bad:?}; `tagteam displaced` lists them")}}),
                "{ids:?}"
            );
        }
    }
    cmd(d.path())
        .args(["displaced", "--purge", "../../tagteam.db", "--yes"])
        .assert()
        .code(1)
        .stdout("")
        .stderr("tagteam: no displaced credential matches \"../../tagteam.db\"; `tagteam displaced` lists them\n");
    for name in [&id, &upper] {
        assert!(
            displaced_dir(d.path())
                .join(format!("{name}.json"))
                .exists(),
            "{name}"
        );
    }
    assert!(
        Env::for_test(d.path())
            .data_dir()
            .join("tagteam.db")
            .exists()
    );
    assert_eq!(listing(d.path())["displaced"][0]["id"], json!(id));
}

#[test]
fn yes_without_purge_is_a_usage_error() {
    let d = tempfile::tempdir().unwrap();
    cmd(d.path())
        .args(["displaced", "--yes"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(format!("tagteam: {YES_ALONE}\n"));
    let out = cmd(d.path())
        .args(["displaced", "--yes", "--json"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "usage", "message": YES_ALONE}})
    );
    // `--purge` takes at least one ID.
    cmd(d.path())
        .args(["displaced", "--purge"])
        .assert()
        .code(2);
}
