//! Claude Code's export payload (§13.3, Decision 11): `export_login` writes it, slim or full,
//! and `import_login` validates it and rebuilds the stored login.

use std::sync::Arc;

use serde_json::{Value, json};
use tagteam_cc::ClaudeCode;
use tagteam_cc::live::Platform;
use tagteam_provider::{FakeKeychain, Provider, ProviderError, StoredLogin};

const API_KEY: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";

fn cc() -> ClaudeCode {
    ClaudeCode::new(Arc::new(FakeKeychain::new()), Platform::MacOs)
}

fn oauth_account(email: &str) -> Value {
    json!({"emailAddress": email, "accountUuid": format!("uuid-{email}"),
        "organizationUuid": "org-1", "organizationName": "Acme", "displayName": "A"})
}

/// A stored OAuth credential with every key Appendix A.4 names, and a 2.1.286 one.
fn stored(rt: &str) -> Value {
    json!({
        "claudeAiOauth": {"accessToken": "at-SENTINEL", "refreshToken": rt,
            "expiresAt": 1_790_003_600_000i64, "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max"},
        "mcpOAuth": {"srv": {"token": "machine-shared"}},
        "pluginSecrets": {"p": "s"},
        "trustedDeviceToken": "device-bound",
        "designOauth": {"t": 1}
    })
}

fn login(cc: &ClaudeCode, kind: &str, secret: &[u8], email: &str) -> StoredLogin {
    StoredLogin {
        kind: kind.into(),
        secret: secret.to_vec(),
        identity: cc.parse_identity(&oauth_account(email)).unwrap(),
    }
}

#[test]
fn a_slim_export_keeps_only_claude_ai_oauth_and_a_full_one_keeps_everything() {
    let cc = cc();
    let l = login(
        &cc,
        "oauth",
        stored("rt-1").to_string().as_bytes(),
        "a@x.co",
    );
    let (identity, slim) = cc.export_login(&l, false).unwrap();
    assert_eq!(
        slim,
        json!({"claudeAiOauth": stored("rt-1")["claudeAiOauth"]})
    );
    let (_, full) = cc.export_login(&l, true).unwrap();
    assert_eq!(full, stored("rt-1"));
    assert_eq!(
        identity,
        json!({"email": "a@x.co", "accountUuid": "uuid-a@x.co", "organizationUuid": "org-1",
            "organizationName": "Acme", "oauthAccount": oauth_account("a@x.co")}),
        "§13.3's example"
    );
}

#[test]
fn an_export_imports_back_as_the_same_login() {
    let cc = cc();
    let l = login(
        &cc,
        "oauth",
        stored("rt-1").to_string().as_bytes(),
        "a@x.co",
    );
    let (identity, credential) = cc.export_login(&l, false).unwrap();
    let back = cc.import_login(&identity, &credential).unwrap();
    assert_eq!(back.kind, "oauth");
    assert_eq!(
        cc.identity_key(&back.identity),
        cc.identity_key(&l.identity)
    );
    assert_eq!(back.identity.raw, oauth_account("a@x.co"));
    assert_eq!(cc.fingerprint(&back.secret), cc.fingerprint(&l.secret));
    assert_eq!(
        serde_json::from_slice::<Value>(&back.secret).unwrap(),
        credential
    );
}

#[test]
fn an_api_key_is_a_string_and_a_setup_token_its_json() {
    let cc = cc();
    let key = login(&cc, "api_key", API_KEY.as_bytes(), "api-key-2@token.local");
    let (_, credential) = cc.export_login(&key, false).unwrap();
    assert_eq!(credential, json!(API_KEY));
    let back = cc
        .import_login(&json!({"email": "api-key-2@token.local"}), &credential)
        .unwrap();
    assert_eq!(
        (back.kind.as_str(), back.secret.as_slice()),
        ("api_key", API_KEY.as_bytes())
    );
    assert_eq!(
        back.identity.raw["accountUuid"], "",
        "a token account's identity"
    );

    let setup =
        json!({"claudeAiOauth": {"accessToken": "sk-ant-oat01-x", "scopes": ["user:inference"]}});
    let l = login(&cc, "setup_token", setup.to_string().as_bytes(), "s@x.co");
    let (identity, credential) = cc.export_login(&l, false).unwrap();
    assert_eq!(credential, setup);
    assert_eq!(
        cc.import_login(&identity, &credential).unwrap().kind,
        "setup_token"
    );
}

/// An `import_login` refusal's message, which must never quote the credential's tokens.
fn refused(identity: Value, credential: Value) -> String {
    match cc().import_login(&identity, &credential) {
        Err(ProviderError::Invalid(m)) => {
            assert!(!m.contains("SENTINEL"), "{m}");
            m
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn import_refuses_what_claude_code_could_not_use_without_quoting_a_token() {
    let identity = json!({"email": "a@x.co", "oauthAccount": oauth_account("a@x.co")});
    let good =
        json!({"claudeAiOauth": {"accessToken": "at-SENTINEL", "refreshToken": "rt-SENTINEL"}});
    let wiped = json!({"claudeAiOauth": {"accessToken": "", "refreshToken": ""}});
    let cases = [
        (
            json!({"oauthAccount": oauth_account("not-an-email")}),
            good.clone(),
            "\"not-an-email\" is not a valid email address",
        ),
        (
            json!({"email": "b@x.co", "oauthAccount": oauth_account("a@x.co")}),
            good.clone(),
            "the identity's email and its oauthAccount name different logins",
        ),
        (json!({}), good.clone(), "the identity has no email"),
        (
            json!("a@x.co"),
            good.clone(),
            "the identity is not a JSON object",
        ),
        (identity.clone(), wiped, "the credential holds no token"),
        (
            identity.clone(),
            json!("sk-ant-oat01-SENTINEL"),
            "a credential string must be an API key",
        ),
        (
            identity.clone(),
            json!({"mcpOAuth": {"t": "SENTINEL"}}),
            "the credential has no claudeAiOauth object",
        ),
        (
            identity,
            json!(42),
            "the credential is neither a JSON object nor an API key",
        ),
    ];
    for (identity, credential, want) in cases {
        assert_eq!(refused(identity, credential), want);
    }
}
