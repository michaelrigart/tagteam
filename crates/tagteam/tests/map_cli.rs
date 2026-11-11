//! §12.7: `map`, `unmap` and `shell-init`, through the real binary. Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use common::{
    cc_profile, cmd, installed, legacy_mapping, login, seed_home, two_accounts, write_config,
};
use serde_json::{Value, json};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{NewAccount, Store};
use tagteam_provider::{Env, FileKeychain, Identity, MARKER_FILE};

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

/// `map --json`'s file entries.
fn config_listed(root: &Path) -> Vec<Value> {
    let out = cmd(root).args(["map", "--json"]).assert().success();
    json_of(&out.get_output().stdout)["configMappings"]
        .as_array()
        .unwrap()
        .clone()
}

/// Where `map` writes: the settings file of `root`'s fixture HOME.
fn config_file(root: &Path) -> PathBuf {
    Env::for_test(root).config_dir().join("config.toml")
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
    assert_eq!(v["mapping"]["source"], "config");
    assert!(listed(d.path()).is_empty(), "the database holds nothing");
    let files = config_listed(d.path());
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], real.as_str());
}

#[test]
fn map_defaults_to_the_current_directory_and_a_new_mapping_replaces_the_old() {
    // §12.7: one mapping per provider per path. The directory is under HOME, so its key is
    // `~/…`.
    let d = tempfile::tempdir().unwrap();
    let (_, b) = two_accounts(d.path());
    let (app, _) = dir(&d.path().join("home"), "work/app");
    let config = config_file(d.path());
    cmd(d.path())
        .current_dir(&app)
        .args(["map", "1"])
        .assert()
        .success()
        .stdout(format!(
            "Mapped ~/work/app to a@x.co (position 1) in {}.\n",
            config.display()
        ));
    cmd(d.path())
        .current_dir(&app)
        .args(["map", "b@x.co"])
        .assert()
        .success();
    let files = config_listed(d.path());
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0]["value"], "b@x.co");
    assert_eq!(files[0]["written"], "~/work/app");
    assert_eq!(files[0]["account"]["id"], b.as_str());
}

#[test]
fn map_lists_every_mapping_in_path_order() {
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    let (_, x_real) = dir(d.path(), "x");
    let (y, y_real) = dir(d.path(), "y");
    legacy_mapping(d.path(), &x_real, &a);
    cmd(d.path()).args(["map", "2"]).arg(&y).assert().success();
    assert_eq!(
        listed_shapes(d.path()),
        vec![
            json!({"path": x_real, "provider": "claude-code", "number": 1, "id": a, "email": "a@x.co"})
        ]
    );
    cmd(d.path()).arg("map").assert().success().stdout(format!(
        "{x_real}  1  a@x.co  (database; `tagteam map` moves it to the file)\n{y_real}  2  b@x.co\n"
    ));
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
    assert!(config_listed(d.path()).is_empty());
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
fn unmap_removes_from_both_sources_and_counts_both() {
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    let (x, real) = dir(d.path(), "x");
    for flags in [&[][..], &["--provider", "claude-code"]] {
        cmd(d.path()).args(["map", "1"]).arg(&x).assert().success();
        legacy_mapping(d.path(), &real, &a);
        let out = cmd(d.path())
            .arg("unmap")
            .arg(&x)
            .args(flags)
            .arg("--json")
            .assert()
            .success();
        assert_eq!(json_of(&out.get_output().stdout)["removed"], 2, "{flags:?}");
        assert!(listed(d.path()).is_empty() && config_listed(d.path()).is_empty());
        let text = fs::read_to_string(config_file(d.path())).unwrap_or_default();
        assert!(!text.contains("mappings"), "{text}");
    }
}

#[test]
fn unmap_takes_the_listed_path_of_a_directory_that_is_gone() {
    // Outside HOME the key is absolute. Under it the key is `~/…`, which expands through HOME
    // as spelled, so a gone directory's path must not be canonicalised around HOME. Either way
    // the path `map` lists is the path that unmaps.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    let (outside, outside_real) = dir(d.path(), "gone");
    let (inside, inside_real) = dir(&d.path().join("home"), "gone");
    for (gone, real, also_in_database) in
        [(outside, outside_real, true), (inside, inside_real, false)]
    {
        cmd(d.path())
            .args(["map", "1"])
            .arg(&gone)
            .assert()
            .success();
        if also_in_database {
            legacy_mapping(d.path(), &real, &a);
        }
        fs::remove_dir(&gone).unwrap();
        let files = config_listed(d.path());
        let listed_path = files[0]["path"].as_str().unwrap().to_owned();
        let out = cmd(d.path())
            .args(["unmap", &listed_path, "--json"])
            .assert()
            .success();
        assert_eq!(
            json_of(&out.get_output().stdout)["removed"],
            1 + u64::from(also_in_database),
            "{listed_path}"
        );
        assert!(listed(d.path()).is_empty() && config_listed(d.path()).is_empty());
    }
}

#[test]
fn unmap_takes_an_entry_that_names_a_file_through_a_symlink() {
    // An entry whose path exists but is no directory is kept as written, and `unmap`'s argument
    // must compare through the same normalisation, however the path resolves.
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let (real, _) = dir(d.path(), "real");
    fs::write(real.join("f"), "").unwrap();
    symlink(&real, d.path().join("link")).unwrap();
    let file = d.path().join("link").join("f");
    let config = write_config(
        d.path(),
        &format!(
            "[provider.claude-code.mappings]\n\"{}\" = \"a@x.co\"\n",
            file.display()
        ),
    );
    let out = cmd(d.path())
        .arg("unmap")
        .arg(&file)
        .arg("--json")
        .assert()
        .success();
    assert_eq!(json_of(&out.get_output().stdout)["removed"], 1);
    let text = fs::read_to_string(&config).unwrap();
    assert!(!text.contains("mappings"), "{text}");
}

#[test]
fn unmap_takes_a_path_whose_ancestor_became_a_file() {
    // `canonicalize` fails with NotADirectory there, not NotFound: the path doctor suggests must
    // still unmap, as a path that is merely gone does.
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let (client, _) = dir(&d.path().join("home"), "work/client");
    let config = write_config(
        d.path(),
        "[provider.claude-code.mappings]\n\"~/work/client\" = \"a@x.co\"\n",
    );
    let work = d.path().join("home").join("work");
    fs::remove_dir_all(&work).unwrap();
    fs::write(&work, "").unwrap();
    let out = cmd(d.path())
        .arg("unmap")
        .arg(&client)
        .arg("--json")
        .assert()
        .success();
    assert_eq!(json_of(&out.get_output().stdout)["removed"], 1);
    let text = fs::read_to_string(&config).unwrap();
    assert!(!text.contains("mappings"), "{text}");
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
    let (a, b) = two_accounts(d.path());
    let (_, x_real) = dir(d.path(), "x");
    let (_, y_real) = dir(d.path(), "y");
    legacy_mapping(d.path(), &x_real, &a);
    legacy_mapping(d.path(), &y_real, &b);
    cmd(d.path()).args(["remove", "1"]).assert().success();
    let left: Vec<Value> = listed(d.path())
        .into_iter()
        .map(|r| r["path"].clone())
        .collect();
    assert_eq!(left, [json!(y_real)]);
}

#[test]
fn removing_an_account_warns_about_config_mappings_that_named_it() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let config = write_config(
        d.path(),
        "[provider.claude-code.mappings]\n\"~/x\" = \"a@x.co\"\n",
    );
    let before = fs::read(&config).unwrap();
    let out = cmd(d.path()).args(["remove", "1"]).assert().success();
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(err.contains("~/x"), "{err}");
    assert!(err.contains("\"a@x.co\""), "{err}");
    assert!(err.contains(&config.display().to_string()), "{err}");
    assert_eq!(fs::read(&config).unwrap(), before);
}

/// `n` accounts that share the email `a@x.co`, in organizations o1, o2 and so on, and the entry
/// `"~/client" = "a@x.co"`, which names them all.
fn accounts_sharing_an_email(root: &Path, n: u32) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    for i in 1..=n {
        login(&env, &kc, "a@x.co", &format!("o{i}"), &format!("rt-{i}"));
        cmd(root).arg("add").assert().success();
    }
    write_config(
        root,
        "[provider.claude-code.mappings]\n\"~/client\" = \"a@x.co\"\n",
    );
}

#[test]
fn removing_an_account_warns_when_an_ambiguous_entry_now_names_the_other() {
    // Two accounts share an email, so the entry ran plain; once one goes, it names the other.
    let d = tempfile::tempdir().unwrap();
    accounts_sharing_an_email(d.path(), 2);
    fs::create_dir_all(d.path().join("home").join("client")).unwrap();
    let out = cmd(d.path()).args(["remove", "1"]).assert().success();
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(err.contains("~/client"), "{err}");
    assert!(err.contains("it now names a@x.co (position 2)"), "{err}");
}

#[test]
fn removing_an_account_warns_about_an_ambiguous_entry_whose_directory_is_missing_here() {
    // The value is judged, not this machine's directory: another machine with `~/client` now
    // launches it as the remaining account.
    let d = tempfile::tempdir().unwrap();
    accounts_sharing_an_email(d.path(), 2);
    assert!(!d.path().join("home").join("client").exists());
    let out = cmd(d.path()).args(["remove", "1"]).assert().success();
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(err.contains("~/client"), "{err}");
    assert!(err.contains("it now names a@x.co (position 2)"), "{err}");
}

#[test]
fn removing_an_account_warns_about_an_entry_that_still_names_several() {
    // Three accounts share an email; once one goes the entry still names two and runs plain,
    // but it named the removed account.
    let d = tempfile::tempdir().unwrap();
    accounts_sharing_an_email(d.path(), 3);
    fs::create_dir_all(d.path().join("home").join("client")).unwrap();
    let out = cmd(d.path()).args(["remove", "1"]).assert().success();
    let err = String::from_utf8(out.get_output().stderr.clone()).unwrap();
    assert!(err.contains("~/client"), "{err}");
    assert!(err.contains("a@x.co (position 1)"), "{err}");
    assert!(
        err.contains("it still names more than one account"),
        "{err}"
    );
}

#[test]
fn unsetting_or_renaming_an_alias_warns_about_config_mappings_that_used_it() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    cmd(d.path())
        .args(["alias", "1", "work"])
        .assert()
        .success();
    let config = write_config(
        d.path(),
        "[provider.claude-code.mappings]\n\"~/x\" = \"work\"\n",
    );
    let before = fs::read(&config).unwrap();
    let stderr = |args: &[&str]| {
        let out = cmd(d.path()).arg("alias").args(args).assert().success();
        String::from_utf8(out.get_output().stderr.clone()).unwrap()
    };
    let err = stderr(&["1", "home"]);
    assert!(err.contains("~/x"), "{err}");
    assert!(err.contains(&config.display().to_string()), "{err}");
    assert_eq!(stderr(&["1", "work"]), "", "it named no account before");
    let err = stderr(&["1", "--unset"]);
    assert!(err.contains("~/x"), "{err}");
    assert_eq!(fs::read(&config).unwrap(), before);
}

#[test]
fn an_alias_change_no_entry_used_says_nothing() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    cmd(d.path())
        .args(["alias", "1", "work"])
        .assert()
        .success();
    write_config(
        d.path(),
        "[provider.claude-code.mappings]\n\"~/x\" = \"work\"\n",
    );
    cmd(d.path())
        .args(["alias", "2", "other"])
        .assert()
        .success()
        .stderr("");
}

#[test]
fn on_a_fresh_machine_map_and_unmap_create_nothing() {
    // §5: a command that changes nothing creates nothing.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    let out = cmd(d.path()).args(["map", "--json"]).assert().success();
    assert_eq!(
        json_of(&out.get_output().stdout),
        json!({"schemaVersion": 1, "mappings": [], "configMappings": []})
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
    let out = cmd(d.path())
        .args(["map", "1", "/x", "--json"])
        .assert()
        .code(1);
    assert_eq!(
        json_of(&out.get_output().stdout)["error"]["type"],
        "no-such-account"
    );
    assert!(!Env::for_test(d.path()).data_dir().exists());
    assert!(!config_file(d.path()).exists());
    assert!(!d.path().join("home/.config").exists());
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
    let rows = config_listed(d.path());
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (&rows[0]["path"], &rows[0]["account"]["number"]),
        (&json!(real), &json!(2))
    );
    // The outer home's file, not the run shell's profile.
    let text = fs::read_to_string(config_file(d.path())).unwrap();
    assert!(text.contains("b@x.co"), "{text}");
}

#[test]
fn map_writes_the_alias_else_the_email() {
    let d = tempfile::tempdir().unwrap();
    two_accounts(d.path());
    let home = d.path().join("home");
    let (x, _) = dir(&home, "x");
    let (y, _) = dir(&home, "y");
    cmd(d.path())
        .args(["alias", "1", "work"])
        .assert()
        .success();
    cmd(d.path()).args(["map", "1"]).arg(&x).assert().success();
    cmd(d.path()).args(["map", "2"]).arg(&y).assert().success();
    let text = fs::read_to_string(config_file(d.path())).unwrap();
    assert!(text.contains("\"~/x\" = \"work\""), "{text}");
    assert!(text.contains("\"~/y\" = \"b@x.co\""), "{text}");
}

#[test]
fn map_refuses_an_api_key_account_an_ambiguous_email_and_an_account_with_neither() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = FileKeychain::new(d.path().join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "o1", "rt-1");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "a@x.co", "o2", "rt-2");
    cmd(d.path()).arg("add").assert().success();
    cmd(d.path())
        .args(["add-token", "sk-ant-api03-key"])
        .assert()
        .success();
    Store::open_existing(&env.data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
        .insert_account(&NewAccount {
            id: &AccountId::from_string("0192-nameless"),
            provider: &ProviderId::new("claude-code"),
            position: 4,
            identity_key: "nameless\n",
            identity: &Identity {
                label: "nameless".into(),
                email: None,
                org_uuid: String::new(),
                org_name: None,
                account_uuid: None,
                raw: json!({}),
            },
            kind: "oauth",
            alias: None,
            login_expires_at: None,
            added_at: 1,
        })
        .unwrap();
    let before = "[ui]\ncolor = \"never\"\n";
    let config = write_config(d.path(), before);
    let (x, _) = dir(d.path(), "x");
    for (position, says) in [
        ("1", "alias"),
        ("2", "alias"),
        ("3", "API-key"),
        ("4", "alias"),
    ] {
        let out = cmd(d.path())
            .args(["map", position])
            .arg(&x)
            .arg("--json")
            .assert()
            .code(1);
        let v = json_of(&out.get_output().stdout);
        assert_eq!(v["error"]["type"], "invalid-input", "{position}: {v}");
        let message = v["error"]["message"].as_str().unwrap();
        assert!(message.contains(says), "{position}: {message}");
        assert_eq!(fs::read_to_string(&config).unwrap(), before, "{position}");
    }
}

#[test]
fn map_deletes_the_database_row_for_the_directory() {
    // The file is the only source for a directory `map` has written.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    let (x, x_real) = dir(d.path(), "x");
    legacy_mapping(d.path(), &x_real, &a);
    cmd(d.path()).args(["map", "2"]).arg(&x).assert().success();
    assert!(listed(d.path()).is_empty());
    assert_eq!(config_listed(d.path()).len(), 1);
}

#[test]
fn map_deletes_only_the_database_row_of_the_accounts_own_provider() {
    // The store does not check the registry, so a row for another provider can be seeded.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    let (x, x_real) = dir(d.path(), "x");
    legacy_mapping(d.path(), &x_real, &a);
    let store = Store::open_existing(&Env::for_test(d.path()).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let other = ProviderId::new("fake-agent");
    let other_id = AccountId::from_string("0192-other");
    store
        .insert_account(&NewAccount {
            id: &other_id,
            provider: &other,
            position: 1,
            identity_key: "other\n",
            identity: &Identity {
                label: "other".into(),
                email: None,
                org_uuid: String::new(),
                org_name: None,
                account_uuid: None,
                raw: json!({}),
            },
            kind: "oauth",
            alias: None,
            login_expires_at: None,
            added_at: 1,
        })
        .unwrap();
    store.set_mapping(&x_real, &other, &other_id, 1).unwrap();
    cmd(d.path()).args(["map", "2"]).arg(&x).assert().success();
    let left = store.mappings().unwrap();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].provider, other);
    assert_eq!(left[0].path, x_real);
}

#[test]
fn the_listing_reports_every_problem() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = FileKeychain::new(d.path().join("keychain"));
    seed_home(&env);
    // 1 and 2 share an email, 3 is alone, 4 is an API key.
    login(&env, &kc, "a@x.co", "o1", "rt-1");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "a@x.co", "o2", "rt-2");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-3");
    cmd(d.path()).arg("add").assert().success();
    cmd(d.path())
        .args(["add-token", "sk-ant-api03-key"])
        .assert()
        .success();
    cmd(d.path())
        .args(["alias", "4", "keyed"])
        .assert()
        .success();
    for name in ["noacct", "amb", "api", "pos", "conf", "fine"] {
        fs::create_dir_all(d.path().join("home").join(name)).unwrap();
    }
    write_config(
        d.path(),
        "[provider.claude-code.mappings]\n\
         \"/no/such/dir\" = \"b@x.co\"\n\
         \"~/noacct\" = \"nobody\"\n\
         \"~/amb\" = \"a@x.co\"\n\
         \"~/api\" = \"keyed\"\n\
         \"~/pos\" = \"2\"\n\
         \"~/conf\" = \"b@x.co\"\n\
         \"~/conf/\" = \"keyed\"\n\
         \"~/fine\" = \"b@x.co\"\n",
    );
    let files = config_listed(d.path());
    let problems: std::collections::BTreeSet<&str> =
        files.iter().filter_map(|f| f["problem"].as_str()).collect();
    assert_eq!(
        problems,
        [
            "missing-directory",
            "no-account",
            "ambiguous",
            "api-key",
            "position",
            "conflict"
        ]
        .into_iter()
        .collect()
    );
    let fine = files.iter().find(|f| f["written"] == "~/fine").unwrap();
    assert!(
        fine.get("problem").is_none() && fine["account"]["number"] == 3,
        "{fine}"
    );
    let human = cmd(d.path()).arg("map").assert().success();
    let human = String::from_utf8(human.get_output().stdout.clone()).unwrap();
    assert!(
        human.contains("-  \"nobody\"  (names no account here)"),
        "{human}"
    );
}

/// A stand-in for `name` in `bin`: it writes each argument it gets, one per line, to
/// `<bin>/<name>.args`.
fn recorder(bin: &Path, name: &str) {
    fs::create_dir_all(bin).unwrap();
    let path = bin.join(name);
    let out = bin.join(format!("{name}.args"));
    common::install_script(
        &path,
        &format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{}'\n",
            out.display()
        ),
    );
}

/// The arguments the stand-in for `name` was given, if it ran.
fn recorded(bin: &Path, name: &str) -> Option<Vec<String>> {
    let text = fs::read_to_string(bin.join(format!("{name}.args"))).ok()?;
    Some(text.lines().map(str::to_owned).collect())
}

/// What `tagteam shell-init <shell>` prints.
fn wrapper(root: &Path, shell: &str) -> String {
    let out = cmd(root).args(["shell-init", shell]).assert().success();
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

/// A call with the arguments a careless wrapper would split, glob, expand or drop.
const CALL: &str = "claude 'a b' '' '*' '$HOME' --json -- \"x\"";
const PASSED: [&str; 7] = ["a b", "", "*", "$HOME", "--json", "--", "x"];

/// Each shell, its flags for running a script without reading any rc file, and the
/// `shell-init` text it runs: POSIX `sh` runs bash's.
const SHELLS: [(&str, &[&str], &str); 4] = [
    ("sh", &["-c"], "bash"),
    ("bash", &["-c"], "bash"),
    ("zsh", &["-f", "-c"], "zsh"),
    ("fish", &["--no-config", "-c"], "fish"),
];

/// Runs each installed shell's wrapper and then `CALL`, with only `bin-<shell>` on `PATH`;
/// a shell that is not installed is skipped, and the test says so.
fn through_each_shell(root: &Path, stand_ins: &[&str]) -> Vec<(&'static str, PathBuf)> {
    let mut ran = Vec::new();
    for (name, flags, init) in SHELLS {
        let Some(shell) = installed(name) else {
            eprintln!("skipped: {name} is not installed");
            continue;
        };
        let bin = root.join(format!("bin-{name}"));
        for stand_in in stand_ins {
            recorder(&bin, stand_in);
        }
        let status = std::process::Command::new(&shell)
            .args(flags)
            .arg(format!("{}\n{CALL}\n", wrapper(root, init)))
            .env_clear()
            .env("PATH", &bin)
            .env("HOME", &bin)
            .status()
            .unwrap();
        assert!(status.success(), "{name}: {status:?}");
        ran.push((name, bin));
    }
    ran
}

#[test]
fn the_wrapper_hands_every_argument_to_tagteam_run_intact() {
    let d = tempfile::tempdir().unwrap();
    let mut expected = vec!["run", "--provider", "claude-code", "--"];
    expected.extend(PASSED);
    for (name, bin) in through_each_shell(d.path(), &["tagteam", "claude"]) {
        assert_eq!(
            recorded(&bin, "tagteam").as_deref(),
            Some(&expected.iter().map(|s| s.to_string()).collect::<Vec<_>>()[..]),
            "{name}"
        );
        assert_eq!(
            recorded(&bin, "claude"),
            None,
            "{name}: tagteam decides, so the wrapper never runs claude itself"
        );
    }
}

#[test]
fn without_tagteam_on_path_the_wrapper_runs_claude_itself() {
    let d = tempfile::tempdir().unwrap();
    for (name, bin) in through_each_shell(d.path(), &["claude"]) {
        assert_eq!(
            recorded(&bin, "claude").as_deref(),
            Some(&PASSED.map(str::to_owned)[..]),
            "{name}"
        );
    }
}

#[test]
fn shell_init_prints_text_never_json_and_knows_its_shells() {
    let d = tempfile::tempdir().unwrap();
    let out = cmd(d.path())
        .args(["shell-init", "zsh", "--json"])
        .assert()
        .code(2);
    assert_eq!(
        json_of(&out.get_output().stdout),
        json!({"schemaVersion": 1, "error": {"type": "usage",
               "message": "shell-init prints shell code; run it without --json"}})
    );
    cmd(d.path())
        .args(["shell-init", "powershell"])
        .assert()
        .code(2);
    cmd(d.path()).arg("shell-init").assert().code(2);
    cmd(d.path())
        .args(["--provider", "nope", "shell-init", "zsh"])
        .assert()
        .code(1)
        .stderr("tagteam: unknown provider \"nope\"\n");
}

#[test]
fn shell_init_needs_no_store_and_creates_nothing() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    let text = wrapper(d.path(), "fish");
    assert!(text.starts_with("function claude "), "{text}");
    assert!(
        fs::read_dir(d.path().join("home"))
            .unwrap()
            .next()
            .is_none(),
        "HOME stays empty"
    );
}

#[test]
fn shell_init_refuses_under_a_marker_that_cannot_be_read() {
    // §12.8, Decision 19: under an unreadable run-shell marker every command but `statusline`
    // refuses, `shell-init` among them, and prints no wrapper.
    let d = tempfile::tempdir().unwrap();
    let profile = Env::for_test(d.path()).data_dir().join("sessions/0192");
    fs::create_dir_all(&profile).unwrap();
    fs::write(
        profile.join(MARKER_FILE),
        "{\"format\": \"tagteam-profile\"",
    )
    .unwrap();
    let out = cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &profile)
        .args(["shell-init", "bash", "--json"])
        .assert()
        .code(1);
    assert_eq!(
        json_of(&out.get_output().stdout)["error"]["type"],
        "run-shell-unreadable",
        "the marker check runs before shell-init's own --json refusal"
    );
    cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &profile)
        .args(["shell-init", "bash"])
        .assert()
        .code(1)
        .stdout("");
}
