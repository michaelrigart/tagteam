//! §15.2: kill the process at each switch step, then prove that the next command recovers it.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::time::{Duration, SystemTime};

use assert_cmd::Command;
use common::{login, seed_home};
use serde_json::{Value, json};
use tagteam_cc::{CcPaths, ItemKind, keychain_account, keychain_service};
use tagteam_provider::splice::get_top_level;
use tagteam_provider::{Env, FileKeychain, Keychain};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        seed_home(&Env::for_test(dir.path()));
        Home { dir }
    }

    fn env(&self) -> Env {
        Env::for_test(self.dir.path())
    }

    fn kc(&self) -> FileKeychain {
        FileKeychain::new(self.dir.path().join("keychain"))
    }

    fn oauth_item(&self) -> (String, String) {
        (
            keychain_service(&self.env(), ItemKind::OAuth),
            keychain_account(&self.env()),
        )
    }

    fn cmd(&self) -> Command {
        common::cmd(self.dir.path())
    }

    fn login(&self, email: &str, rt: &str) {
        login(&self.env(), &self.kc(), email, "", rt);
    }

    /// Simulates Claude Code rotating the live credential on its own, without a `login` (a
    /// new `oauthAccount`): only the Keychain item changes.
    fn set_live_rt(&self, rt: &str) {
        let (svc, acct) = self.oauth_item();
        let cred = json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": rt}});
        self.kc()
            .upsert(&svc, &acct, cred.to_string().as_bytes())
            .unwrap();
    }

    fn live(&self) -> (String, String) {
        let doc = fs::read(self.env().home.join(".claude.json")).unwrap();
        let email = get_top_level(&doc, "oauthAccount").unwrap().unwrap()["emailAddress"]
            .as_str()
            .unwrap()
            .to_owned();
        let (svc, acct) = self.oauth_item();
        let cred: Value =
            serde_json::from_slice(&self.kc().find(&svc, &acct).present().unwrap()).unwrap();
        (
            email,
            cred["claudeAiOauth"]["refreshToken"]
                .as_str()
                .unwrap()
                .to_owned(),
        )
    }

    /// A killed process leaves CC's mkdir locks behind; age them past their staleness.
    fn age_cc_locks(&self) {
        let p = CcPaths::resolve(&self.env());
        for lock in [
            p.refresh_lock.clone(),
            p.legacy_lock(),
            p.config_lock.clone(),
        ] {
            if lock.exists() {
                fs::File::open(&lock)
                    .unwrap()
                    .set_modified(SystemTime::now() - Duration::from_secs(120))
                    .unwrap();
            }
        }
    }

    fn two_accounts(&self) {
        self.login("a@x.co", "rt-a");
        self.cmd().arg("add").assert().success();
        self.login("b@x.co", "rt-b");
        self.cmd().arg("add").assert().success();
    }
}

#[test]
fn a_kill_at_any_step_is_recovered_by_the_next_command() {
    for (point, lands) in [
        ("after-journal", false),
        ("after-credential", true),
        ("after-identity", true),
    ] {
        let h = Home::new();
        h.two_accounts();
        h.cmd()
            .args(["switch", "1"])
            .env("TAGTEAM_TEST_CRASH_AT", point)
            .assert()
            .failure();
        h.age_cc_locks();
        h.cmd().args(["disable", "2"]).assert().success(); // any mutating command recovers
        let expected = if lands {
            ("a@x.co", "rt-a")
        } else {
            ("b@x.co", "rt-b")
        };
        let (email, rt) = h.live();
        assert_eq!((email.as_str(), rt.as_str()), expected, "{point}");
        let list: Value = serde_json::from_slice(
            &h.cmd()
                .args(["list", "--json"])
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .unwrap();
        assert_eq!(
            list["activeAccountNumber"],
            json!(if lands { 1 } else { 2 }),
            "{point}"
        );
        h.cmd().args(["enable", "2"]).assert().success();
        h.cmd().arg("switch").assert().success(); // no interrupted switch is left behind
    }
}

#[test]
fn a_cc_rotation_after_the_kill_needs_force() {
    let h = Home::new();
    h.two_accounts();
    h.cmd()
        .args(["switch", "1"])
        .env("TAGTEAM_TEST_CRASH_AT", "after-credential")
        .assert()
        .failure();
    h.age_cc_locks();
    h.set_live_rt("rt-a-rotated-by-cc");
    h.cmd()
        .args(["switch", "2"])
        .assert()
        .code(1)
        .stderr(predicates::str::contains("interrupted switch"));
    h.cmd().args(["switch", "2", "--force"]).assert().success();
    assert_eq!(h.live(), ("b@x.co".to_owned(), "rt-b".to_owned()));
}
