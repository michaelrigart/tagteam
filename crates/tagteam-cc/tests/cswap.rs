//! cswap's version 1 export (Decision 14): its accounts read as records for Claude Code.

use std::sync::Arc;

use serde_json::{Value, json};
use tagteam_cc::ClaudeCode;
use tagteam_cc::cswap::read_cswap_v1;
use tagteam_cc::live::Platform;
use tagteam_core::CLAUDE_CODE;
use tagteam_provider::{FakeKeychain, Provider};

const API_KEY: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
const SETUP_TOKEN: &str = "sk-ant-oat01-setup";

fn credential(rt: &str) -> Value {
    json!({"claudeAiOauth": {"accessToken": format!("at-{rt}"), "refreshToken": rt, "expiresAt": 1_790_003_600_000i64}})
}

/// A cswap version 1 export (inventory §8.1) of `accounts`.
fn cswap(accounts: Value) -> Value {
    json!({"version": 1, "exportedAt": "2026-09-20T10:00:00Z", "exportedFrom": "macos",
        "swapVersion": "0.27.0b1", "encrypted": false, "activeAccountNumber": 1,
        "accounts": accounts})
}

/// The token-account identity cswap records (inventory §2.2).
fn token_account(email: &str) -> Value {
    json!({"emailAddress": email, "accountUuid": "", "organizationUuid": null, "organizationName": null})
}

/// An OAuth account, an API-key account and a setup-token account, as cswap exports them.
fn cswap_three() -> Value {
    cswap(json!([
        {"number": 1, "email": "a@x.co", "uuid": "uuid-a", "organizationUuid": "org-1",
         "organizationName": "Acme", "added": "2026-09-01T08:00:00.123456+00:00",
         "credentials": credential("rt-a"),
         "config": {"oauthAccount": {"emailAddress": "a@x.co", "accountUuid": "uuid-a",
            "organizationUuid": "org-1", "organizationName": "Acme"}},
         "alias": "main"},
        {"number": 2, "email": "api-key-2@token.local", "uuid": "", "organizationUuid": null,
         "organizationName": null, "added": "2026-09-02T08:00:00", "credentials": API_KEY,
         "config": {"oauthAccount": token_account("api-key-2@token.local")}, "kind": "api_key"},
        {"number": 5, "email": "setup-token-5@token.local", "uuid": "", "organizationUuid": null,
         "organizationName": null,
         "credentials": {"claudeAiOauth": {"accessToken": SETUP_TOKEN, "scopes": ["user:inference"]}},
         "config": {"oauthAccount": token_account("setup-token-5@token.local")}}
    ]))
}

#[test]
fn a_cswap_export_maps_its_oauth_api_key_and_setup_token_accounts_to_claude_code() {
    let records = read_cswap_v1(&cswap_three()).unwrap();
    assert_eq!(records.len(), 3);
    for r in &records {
        assert_eq!(r.provider.as_str(), CLAUDE_CODE);
    }
    let positions: Vec<u32> = records.iter().map(|r| r.position).collect();
    assert_eq!(positions, [1, 2, 5]);
    let kinds: Vec<Option<&str>> = records.iter().map(|r| r.kind.as_deref()).collect();
    assert_eq!(kinds, [None, Some("api_key"), None]);
    assert_eq!(records[0].alias.as_deref(), Some("main"));
    assert_eq!(records[0].added_at, Some(1_788_249_600_000));
    assert_eq!(
        records[1].added_at, None,
        "a time with no offset is dropped"
    );
    assert_eq!(
        records[0].identity,
        json!({"email": "a@x.co", "accountUuid": "uuid-a", "organizationUuid": "org-1",
            "organizationName": "Acme", "oauthAccount": {"emailAddress": "a@x.co",
            "accountUuid": "uuid-a", "organizationUuid": "org-1", "organizationName": "Acme"}})
    );
    assert_eq!(records[1].credential, json!(API_KEY));

    // Claude Code's own validation tells the three kinds apart (§7.1).
    let cc = ClaudeCode::new(Arc::new(FakeKeychain::new()), Platform::MacOs);
    let imported: Vec<String> = records
        .iter()
        .map(|r| cc.import_login(&r.identity, &r.credential).unwrap().kind)
        .collect();
    assert_eq!(imported, ["oauth", "api_key", "setup_token"]);
}

#[test]
fn a_cswap_export_that_is_encrypted_or_not_version_1_is_refused_by_name() {
    let mut encrypted = cswap_three();
    encrypted["encrypted"] = json!(true);
    let err = read_cswap_v1(&encrypted).unwrap_err();
    assert_eq!(
        err.to_string(),
        "the cswap export is encrypted (encrypted: true), which tagteam cannot read"
    );
    let mut no_config = cswap_three();
    no_config["accounts"][1]
        .as_object_mut()
        .unwrap()
        .remove("config");
    let err = read_cswap_v1(&no_config).unwrap_err();
    assert_eq!(
        err.to_string(),
        "account 2: config.oauthAccount must be a JSON object"
    );
    assert!(!err.to_string().contains(API_KEY));
}
