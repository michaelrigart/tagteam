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
    &["map"],
    &["map", "1", "/"],
    &["unmap", "/"],
    &["shell-init", "zsh"],
    // No `--` form: the loop appends `--json`, which after `--` would be the agent's.
    &["run"],
    &["run", "1"],
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
fn a_marker_that_is_a_link_to_nothing_refuses_like_an_unreadable_one() {
    // §4.3, §12.8: a dangling marker link is no absent marker, so the profile is never taken
    // for the default home. `switch` refuses naming it; the status bar shows nothing.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_fresh_accounts(d.path());
    let dir = profile_dir(d.path(), &a);
    fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(d.path().join("nowhere"), dir.join(MARKER_FILE)).unwrap();
    let marker = dir.join(MARKER_FILE).display().to_string();
    let out = cmd(d.path())
        .env("CLAUDE_CONFIG_DIR", &dir)
        .args(["switch", "1", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = json_of(&out);
    assert_eq!(v["error"]["type"], "run-shell-unreadable", "{v}");
    assert!(
        v["error"]["message"].as_str().unwrap().contains(&marker),
        "{v}"
    );
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

/// Task 14: `list` and `status` in and around sessions (§12.8, §13.1, §13.2).
mod sessions_in_list_and_status {
    use std::fs;
    use std::path::Path;

    use serde_json::{Value, json};
    use tagteam_core::WindowKind;

    use crate::common::{
        cc_profile, cmd, hold_launch, now_epoch_s, record_reading, two_fresh_accounts, usage_window,
    };

    /// `a@x.co` at position 1 and `b@x.co` at position 2 (live), each with a reading taken just
    /// now: 5h at 9 % and 7d at 77 %, with no resets. Nothing is due for 180 s (§8.3), so
    /// neither `list` nor `status` sends a request.
    fn read_now(root: &Path) -> (String, String) {
        let (a, b) = two_fresh_accounts(root);
        let now = now_epoch_s();
        for id in [&a, &b] {
            record_reading(
                root,
                id,
                now,
                &[
                    usage_window("5h", "5h", WindowKind::Short, 9.0, None, None),
                    usage_window("7d", "7d", WindowKind::Long, 77.0, None, None),
                ],
            );
        }
        (a, b)
    }

    /// `tagteam <args>`, inside the run shell whose profile `shell` spells when given. It must
    /// succeed with nothing on stderr.
    fn run(root: &Path, shell: Option<&str>, args: &[&str]) -> String {
        let mut c = cmd(root);
        if let Some(dir) = shell {
            c.env("CLAUDE_CONFIG_DIR", dir);
        }
        let out = c.args(args).assert().success().get_output().clone();
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "{args:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    fn json_of(root: &Path, shell: Option<&str>, args: &[&str]) -> Value {
        serde_json::from_str(&run(root, shell, args)).unwrap()
    }

    fn keys(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    /// The table with no account in a session: today's layout.
    const QUIET: &str = concat!(
        "    #  ACCOUNT  5H    7D    SPEND  AGE\n",
        "    1  a@x.co     9%   77%  —      <1m\n",
        " *  2  b@x.co     9%   77%  —      <1m\n",
    );

    #[test]
    fn list_in_a_run_shell_marks_the_sessions_account_and_keeps_the_default_login_live() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = read_now(d.path());
        let (profile, shell) = cc_profile(d.path(), &a);
        assert_eq!(
            run(d.path(), None, &["list"]),
            QUIET,
            "a quiescent profile changes nothing"
        );

        let _session = hold_launch(&profile);
        assert_eq!(
            run(d.path(), Some(&shell), &["list"]),
            concat!(
                "    #    ACCOUNT  5H    7D    SPEND  AGE\n",
                "    1 ▶  a@x.co     9%   77%  —      <1m  this\n",
                " *  2    b@x.co     9%   77%  —      <1m\n",
            )
        );
        let v = json_of(d.path(), Some(&shell), &["list", "--json"]);
        assert_eq!(
            v["activeAccountNumber"], 2,
            "the default home's login is live"
        );
        let rows = v["accounts"].as_array().unwrap();
        let first = keys(&rows[0]);
        assert_eq!(
            (rows[0]["id"].as_str(), &first[first.len() - 2..]),
            (Some(a.as_str()), &["loginExpiresAt", "inSession"][..])
        );
        assert_eq!(rows[0]["inSession"], json!(true));
        assert_eq!(
            (rows[1]["id"].as_str(), rows[1].get("inSession")),
            (Some(b.as_str()), None)
        );
    }

    #[test]
    fn status_in_a_run_shell_names_the_sessions_account_in_text_and_json() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = read_now(d.path());
        let (profile, shell) = cc_profile(d.path(), &a);
        let _session = hold_launch(&profile);
        assert_eq!(
            run(d.path(), Some(&shell), &["status"]),
            "Live: b@x.co (position 2 of 2)\n  5h 9% · 7d 77% · <1m old\nThis session: a@x.co (position 1)\n"
        );
        let v = json_of(d.path(), Some(&shell), &["status", "--json"]);
        assert_eq!(
            keys(&v),
            [
                "schemaVersion",
                "provider",
                "active",
                "totalManagedAccounts",
                "session"
            ]
        );
        assert_eq!(
            v["session"],
            json!({"number": 1, "position": 1, "id": a.as_str(), "email": "a@x.co"})
        );
        assert_eq!(
            (v["active"]["id"].as_str(), v["active"]["managed"].as_bool()),
            (Some(b.as_str()), Some(true))
        );
        assert!(v["active"].get("inSession").is_none(), "{v}");
    }

    #[test]
    fn a_run_shell_of_an_account_tagteam_does_not_manage_says_so() {
        let d = tempfile::tempdir().unwrap();
        read_now(d.path());
        let (profile, shell) = cc_profile(d.path(), "0192-not-managed");
        // The login Claude Code keeps in the profile: only `status`'s text reads it.
        let login = json!({"oauthAccount": {"emailAddress": "c@x.co", "organizationUuid": "", "accountUuid": "uuid-c"}});
        fs::write(profile.join(".claude.json"), login.to_string()).unwrap();
        let _session = hold_launch(&profile);
        assert_eq!(
            run(d.path(), Some(&shell), &["status"]),
            "Live: b@x.co (position 2 of 2)\n  5h 9% · 7d 77% · <1m old\nThis session: c@x.co (not managed by tagteam)\n"
        );
        let v = json_of(d.path(), Some(&shell), &["status", "--json"]);
        assert_eq!(v.get("session"), Some(&Value::Null));
        assert_eq!(
            run(d.path(), Some(&shell), &["list"]),
            QUIET,
            "no row is this session's"
        );
    }

    #[test]
    fn outside_a_run_shell_a_session_shows_without_this_and_status_names_none() {
        let d = tempfile::tempdir().unwrap();
        let (a, _b) = read_now(d.path());
        let (profile, _shell) = cc_profile(d.path(), &a);
        let _session = hold_launch(&profile);
        assert_eq!(
            run(d.path(), None, &["list"]),
            concat!(
                "    #    ACCOUNT  5H    7D    SPEND  AGE\n",
                "    1 ▶  a@x.co     9%   77%  —      <1m\n",
                " *  2    b@x.co     9%   77%  —      <1m\n",
            )
        );
        assert_eq!(
            run(d.path(), None, &["status"]),
            "Live: b@x.co (position 2 of 2)\n  5h 9% · 7d 77% · <1m old\n"
        );
        let v = json_of(d.path(), None, &["status", "--json"]);
        assert!(v.get("session").is_none(), "{v}");
    }
}

/// Task 15: `statusline` inside a run shell (§12.8, §13.5; Review Focus 4).
mod statusline_in_a_run_shell {
    use std::fs;
    use std::path::Path;

    use serde_json::{Value, json};
    use tagteam_core::{AccountId, WindowKind};
    use tagteam_engine::store::Store;
    use tagteam_provider::{Env, MARKER_FILE};

    use crate::common::{
        cc_profile, cmd, hold_launch, now_epoch_s, record_reading, two_fresh_accounts, usage_window,
    };

    /// `a@x.co` (position 1) read at 5h 31 % and 7d 12 %, and `b@x.co` (position 2, live) at
    /// 9 % and 77 %, both just now.
    fn read_now(root: &Path) -> (String, String) {
        let (a, b) = two_fresh_accounts(root);
        let now = now_epoch_s();
        for (id, five, seven) in [(&a, 31.0, 12.0), (&b, 9.0, 77.0)] {
            record_reading(
                root,
                id,
                now,
                &[
                    usage_window("5h", "5h", WindowKind::Short, five, None, None),
                    usage_window("7d", "7d", WindowKind::Long, seven, None, None),
                ],
            );
        }
        (a, b)
    }

    /// `tagteam statusline --no-color`, inside the run shell `shell` spells when given.
    fn statusline(root: &Path, shell: Option<&str>) -> assert_cmd::Command {
        let mut c = cmd(root);
        c.args(["statusline", "--no-color"]);
        if let Some(dir) = shell {
            c.env("CLAUDE_CONFIG_DIR", dir);
        }
        c
    }

    #[test]
    fn the_line_is_the_sessions_account_named_by_its_marker() {
        let d = tempfile::tempdir().unwrap();
        let (a, _b) = read_now(d.path());
        statusline(d.path(), None)
            .assert()
            .success()
            .stdout("b · 5h 9% · 7d 77%\n")
            .stderr("");
        let (profile, shell) = cc_profile(d.path(), &a);
        // CC's own file in the profile, naming the default login: a line from it would be b's.
        let login = json!({"oauthAccount": {"emailAddress": "b@x.co", "organizationUuid": ""}});
        fs::write(profile.join(".claude.json"), login.to_string()).unwrap();
        let _session = hold_launch(&profile);
        statusline(d.path(), Some(&shell))
            .assert()
            .success()
            .stdout("a · 5h 31% · 7d 12%\n")
            .stderr("");
    }

    #[test]
    fn a_run_shell_whose_account_was_removed_meanwhile_prints_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (a, _b) = read_now(d.path());
        let (profile, shell) = cc_profile(d.path(), &a);
        let _session = hold_launch(&profile);
        // The row is gone and the profile stays, as a store reset leaves it.
        Store::open_existing(&Env::for_test(d.path()).data_dir().join("tagteam.db"))
            .unwrap()
            .unwrap()
            .delete_account(&AccountId::from_string(a.as_str()))
            .unwrap();
        statusline(d.path(), Some(&shell))
            .assert()
            .success()
            .stdout("")
            .stderr("");
    }

    #[test]
    fn a_corrupt_marker_prints_nothing_while_every_other_command_names_it_and_refuses() {
        let d = tempfile::tempdir().unwrap();
        let (a, _b) = read_now(d.path());
        let (profile, shell) = cc_profile(d.path(), &a);
        fs::write(
            profile.join(MARKER_FILE),
            b"{\"format\": \"tagteam-profile\", \"version\": ",
        )
        .unwrap();
        statusline(d.path(), Some(&shell))
            .assert()
            .success()
            .stdout("")
            .stderr("");
        cmd(d.path())
            .env("CLAUDE_CONFIG_DIR", &shell)
            .arg("status")
            .assert()
            .code(1)
            .stdout("")
            .stderr(predicates::str::contains(MARKER_FILE));
        let out = cmd(d.path())
            .env("CLAUDE_CONFIG_DIR", &shell)
            .args(["status", "--json"])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["error"]["type"], "run-shell-unreadable");
    }
}
