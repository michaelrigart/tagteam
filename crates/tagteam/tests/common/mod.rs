//! Shared by the in-process tests (`app.rs`) and the binary tests (`cli.rs`, `kill.rs`). Each
//! test file is its own crate and uses only part of this module, so an item unused by one of
//! them is not dead code overall.
#![allow(dead_code)]

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use serde_json::json;
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_provider::splice::replace_top_level;
use tagteam_provider::{Env, Keychain};

/// Appendix A.3's refusal, pinned verbatim: its wording is part of the user-facing contract.
pub const LOCKED: &str = "the login keychain is locked (common over SSH); run `security unlock-keychain ~/Library/Keychains/login.keychain-db`, then retry";

/// The binary in an isolated environment rooted at `root`, with its file-backed Keychain.
pub fn std_cmd(root: &Path) -> std::process::Command {
    let mut c = std::process::Command::new(assert_cmd::cargo::cargo_bin("tagteam"));
    c.env_clear()
        .env("HOME", root.join("home"))
        .env("USER", "tester")
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("TAGTEAM_TEST_KEYCHAIN_DIR", root.join("keychain"))
        .env("TAGTEAM_TEST_PLATFORM", "macos");
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
