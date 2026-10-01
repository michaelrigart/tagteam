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
use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, PollPlan, ProviderId, Window, WindowKind};
use tagteam_engine::store::{Eligibility, Reserve, Store};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::splice::replace_top_level;
use tagteam_provider::{Env, FileKeychain, Keychain};

/// Appendix A.3's refusal, pinned verbatim: its wording is part of the user-facing contract.
pub const LOCKED: &str = "the login keychain is locked (common over SSH); run `security unlock-keychain ~/Library/Keychains/login.keychain-db`, then retry";

/// Where every endpoint points unless a test starts a `MockServer`: a local port nothing
/// listens on, so a request fails at once as `PreSend` and no test reaches the network.
pub const OFFLINE_API_BASE: &str = "http://127.0.0.1:9";

/// Now, in epoch seconds.
pub fn now_epoch_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

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

/// `a@x.co` at position 1 and `b@x.co` at position 2 (live), both added through the binary with
/// every endpoint offline (`std_cmd`'s default).
fn add_two_accounts(root: &Path) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(root).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(root).arg("add").assert().success();
}

/// The two accounts of `add_two_accounts`, with their ids read from `list`.
pub fn two_accounts(root: &Path) -> (String, String) {
    add_two_accounts(root);
    let out = cmd(root).args(["list", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = |i: usize| v["accounts"][i]["id"].as_str().unwrap().to_owned();
    (id(0), id(1))
}

/// The accounts of `two_accounts`, with the ids read from the store rather than from `list`.
/// `list` collects usage, and with every endpoint offline it would record a failed fetch, and
/// its backoff, before the test seeds its readings.
pub fn two_fresh_accounts(root: &Path) -> (String, String) {
    add_two_accounts(root);
    let store = Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let rows = store.accounts(&ProviderId::new(CLAUDE_CODE)).unwrap();
    (
        rows[0].id.as_str().to_owned(),
        rows[1].id.as_str().to_owned(),
    )
}

/// A window as a provider reports it, without provider detail.
pub fn usage_window(
    key: &str,
    label: &str,
    kind: WindowKind,
    pct: f64,
    resets_at: Option<i64>,
    period_s: Option<i64>,
) -> Window {
    Window {
        key: key.to_owned(),
        label: label.to_owned(),
        kind,
        pct,
        resets_at,
        period_s,
        detail: None,
    }
}

/// Records `windows` as account `id`'s reading taken at `at_s`, through the store's own write
/// path, as a fetch would: reserve (§8.3 phase 1), then record (phase 3), which writes
/// `usage_state` and one `usage_samples` row per window. One account's readings go in time
/// order, at least 180 s apart (the on-demand rule's minimum age).
pub fn record_reading(root: &Path, id: &str, at_s: i64, windows: &[Window]) {
    let store = Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let row = store.account(&AccountId::from_string(id)).unwrap().unwrap();
    let reservation = match store
        .reserve_usage(
            &row,
            at_s * 1000,
            Eligibility::OnDemand,
            &PollBudget::STANDARD,
        )
        .unwrap()
    {
        Reserve::Reserved(r) => r,
        other => panic!("no reservation for a reading at {at_s}: {other:?}"),
    };
    let plan = PollPlan {
        interval_s: 300,
        next_poll_at: at_s + 300,
    };
    assert!(
        store
            .record_usage(&reservation, windows, at_s, &plan, 180)
            .unwrap(),
        "the record was fenced out"
    );
}

/// Records `readings` readings of account `id`, `spacing_s` apart, the last at `last_s`, each
/// with the windows `windows_at(taken_at)` gives: a long history as fetches would have left it,
/// through the store's own write path on one connection. `spacing_s` must keep inside the hourly
/// budget (§8.6: 20 requests in 3 660 s, so at least 183 s) and the readings in time order.
pub fn record_history(
    root: &Path,
    id: &str,
    last_s: i64,
    readings: usize,
    spacing_s: i64,
    windows_at: &dyn Fn(i64) -> Vec<Window>,
) {
    let store = Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let row = store.account(&AccountId::from_string(id)).unwrap().unwrap();
    for i in (0..readings).rev() {
        let at_s = last_s - i as i64 * spacing_s;
        let Reserve::Reserved(reservation) = store
            .reserve_usage(
                &row,
                at_s * 1000,
                Eligibility::Scheduled,
                &PollBudget::STANDARD,
            )
            .unwrap()
        else {
            panic!("no reservation for a reading at {at_s}");
        };
        let plan = PollPlan {
            interval_s: spacing_s,
            next_poll_at: at_s + spacing_s,
        };
        assert!(
            store
                .record_usage(&reservation, &windows_at(at_s), at_s, &plan, 180)
                .unwrap(),
            "the record was fenced out"
        );
    }
}

/// Pads `~/.claude.json` with Claude Code's per-project state to about `bytes` bytes, as a
/// machine that has run it for months has: what `statusline` must parse on a cache miss.
pub fn bloat_claude_json(root: &Path, bytes: usize) {
    let path = Env::for_test(root).home.join(".claude.json");
    let entry = |n: usize| {
        json!({
            "allowedTools": [],
            "history": (0..8).map(|h| json!({
                "display": format!("a prompt typed in project {n}, number {h}, long enough to look real"),
                "pastedContents": {},
            })).collect::<Vec<_>>(),
            "mcpServers": {},
            "lastCost": 1.25,
            "lastSessionId": format!("00000000-0000-4000-8000-{n:012}"),
        })
    };
    let each = serde_json::to_vec(&entry(0)).unwrap().len() + 40;
    let projects: serde_json::Map<String, Value> = (0..bytes / each + 1)
        .map(|n| (format!("/Users/tester/Code/project-{n}"), entry(n)))
        .collect();
    let doc = fs::read(&path).unwrap();
    fs::write(
        &path,
        replace_top_level(&doc, "projects", &Value::Object(projects)).unwrap(),
    )
    .unwrap();
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
