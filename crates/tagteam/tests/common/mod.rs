//! Shared by the in-process tests (`app.rs`) and the binary tests (`cli.rs`, `kill.rs`). Each
//! test file is its own crate and uses only part of this module, so an item unused by one of
//! them is not dead code overall.
#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use serde_json::{Value, json};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::splice::replace_top_level;
use tagteam_provider::{Env, FileKeychain, Keychain};

/// Appendix A.3's refusal, pinned verbatim: its wording is part of the user-facing contract.
pub const LOCKED: &str = "the login keychain is locked (common over SSH); run `security unlock-keychain ~/Library/Keychains/login.keychain-db`, then retry";

/// Where every endpoint points unless a test starts a `MockServer`: a local port nothing
/// listens on, so a request fails at once as `PreSend` and no test reaches the network.
pub const OFFLINE_API_BASE: &str = "http://127.0.0.1:9";

/// The binary in an isolated environment rooted at `root`, with its file-backed Keychain.
pub fn std_cmd(root: &Path) -> std::process::Command {
    let mut c = std::process::Command::new(assert_cmd::cargo::cargo_bin("tagteam"));
    c.env_clear()
        .env("HOME", root.join("home"))
        .env("USER", "tester")
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("TAGTEAM_TEST_KEYCHAIN_DIR", root.join("keychain"))
        .env("TAGTEAM_TEST_PLATFORM", "macos")
        .env("TAGTEAM_TEST_API_BASE", OFFLINE_API_BASE);
    c
}

pub fn cmd(root: &Path) -> Command {
    Command::from_std(std_cmd(root))
}

/// A home where Claude Code has run, with nobody logged in.
pub fn seed_home(env: &Env) {
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    fs::write(env.home.join(".claude.json"), "{\n  \"userID\": \"u\"\n}\n").unwrap();
}

/// What `claude /login` leaves behind, for a login in organization `org` ("" is personal): its
/// `oauthAccount`, and its credential in the Keychain item Claude Code reads. Logging in again
/// with the same identity and a new `rt` is Claude Code rotating the credential.
pub fn login(env: &Env, kc: &dyn Keychain, email: &str, org: &str, rt: &str) {
    let path = env.home.join(".claude.json");
    let doc = fs::read(&path).unwrap();
    let acct = json!({"emailAddress": email, "organizationUuid": org, "accountUuid": format!("uuid-{email}-{org}")});
    fs::write(
        &path,
        replace_top_level(&doc, "oauthAccount", &acct).unwrap(),
    )
    .unwrap();
    let cred = json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": rt, "refreshTokenExpiresAt": 1_797_000_000_000i64}});
    kc.upsert(
        &keychain_service(env, ItemKind::OAuth),
        &keychain_account(env),
        cred.to_string().as_bytes(),
    )
    .unwrap();
}

/// `a@x.co` at position 1 and `b@x.co` at position 2, both added through the binary with every
/// endpoint offline (`std_cmd`'s default); `b` is live. Returns their ids.
pub fn two_accounts(root: &Path) -> (String, String) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(root).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(root).arg("add").assert().success();
    let out = cmd(root).args(["list", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = |i: usize| v["accounts"][i]["id"].as_str().unwrap().to_owned();
    (id(0), id(1))
}

/// Rewrites account `id`'s vault copy so its access token expires `in_ms` from now: inside
/// the 10-minute freshen window (§7.2) when `in_ms` is below 600 000.
pub fn expire_vault(root: &Path, id: &str, in_ms: i64) {
    let kc = FileKeychain::new(root.join("keychain"));
    let mut v: Value = serde_json::from_slice(&kc.find(SERVICE, id).present().unwrap()).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    v["claudeAiOauth"]["expiresAt"] = json!(now + in_ms);
    kc.upsert(SERVICE, id, v.to_string().as_bytes()).unwrap();
}

/// The email of the `oauthAccount` Claude Code is logged in as.
pub fn live_email(root: &Path) -> String {
    let config: Value =
        serde_json::from_slice(&fs::read(root.join("home/.claude.json")).unwrap()).unwrap();
    config["oauthAccount"]["emailAddress"]
        .as_str()
        .unwrap()
        .to_owned()
}
