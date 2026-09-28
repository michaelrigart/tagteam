use serde_json::{Map, Value, json};
use tagteam_core::Fingerprint;
use tagteam_provider::{Identity, ProviderError};

/// Taken from the live credential on activation, absence included (Appendix A.4, B.9).
pub const MACHINE_SHARED_KEYS: [&str; 5] = [
    "mcpOAuth",
    "mcpOAuthClientConfig",
    "mcpXaaIdp",
    "mcpXaaIdpConfig",
    "pluginSecrets",
];
pub const KIND_OAUTH: &str = "oauth";
pub const KIND_SETUP_TOKEN: &str = "setup_token";
pub const KIND_API_KEY: &str = "api_key";
pub const KINDS: [&str; 3] = [KIND_OAUTH, KIND_SETUP_TOKEN, KIND_API_KEY];

/// §7.1: `trim().starts_with("sk-ant-api") && !starts_with('{')`.
pub fn is_api_key(bytes: &[u8]) -> bool {
    let s = String::from_utf8_lossy(bytes);
    let t = s.trim();
    t.starts_with("sk-ant-api") && !t.starts_with('{')
}

fn oauth_obj(bytes: &[u8]) -> Option<Map<String, Value>> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    v.get("claudeAiOauth")?.as_object().cloned()
}

fn token<'a>(o: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    o.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Precondition: callers must first reject bytes that [`is_wiped`] flags or that have no
/// [`fingerprint`]. Everything that isn't an API key and isn't an OAuth blob carrying a
/// refresh token — a wiped blob, `{}`, garbage, `{"mcpOAuth":{}}` — falls through to
/// `KIND_SETUP_TOKEN`; this function has no "unrecognised" answer.
pub fn classify(bytes: &[u8]) -> &'static str {
    if is_api_key(bytes) {
        return KIND_API_KEY;
    }
    match oauth_obj(bytes) {
        Some(o) if token(&o, "refreshToken").is_some() => KIND_OAUTH,
        _ => KIND_SETUP_TOKEN,
    }
}

/// §2 "Generation": the refresh token, else the access token, else the key itself.
pub fn fingerprint(bytes: &[u8]) -> Option<Fingerprint> {
    if is_api_key(bytes) {
        let s = String::from_utf8_lossy(bytes);
        return Some(Fingerprint::of_secret(s.trim().as_bytes()));
    }
    let o = oauth_obj(bytes)?;
    token(&o, "refreshToken")
        .or_else(|| token(&o, "accessToken"))
        .map(|t| Fingerprint::of_secret(t.as_bytes()))
}

pub fn has_refresh_token(bytes: &[u8]) -> bool {
    oauth_obj(bytes).is_some_and(|o| token(&o, "refreshToken").is_some())
}

/// Both tokens empty: CC's reaction to `invalid_grant`.
pub fn is_wiped(bytes: &[u8]) -> bool {
    oauth_obj(bytes)
        .is_some_and(|o| token(&o, "accessToken").is_none() && token(&o, "refreshToken").is_none())
}

pub fn login_expires_at(bytes: &[u8]) -> Option<i64> {
    oauth_obj(bytes)?.get("refreshTokenExpiresAt")?.as_i64()
}

/// §9.4 step 5: account-scoped keys from the target, machine-shared keys from the live
/// credential (absence included); with no live JSON credential, none at all.
pub fn compose(target: &[u8], live: Option<&Map<String, Value>>) -> Result<Vec<u8>, ProviderError> {
    let mut out: Map<String, Value> = match serde_json::from_slice::<Value>(target) {
        Ok(Value::Object(o)) => o,
        _ => {
            return Err(ProviderError::Invalid(
                "the stored credential is not a JSON object".into(),
            ));
        }
    };
    for k in MACHINE_SHARED_KEYS {
        out.shift_remove(k);
    }
    if let Some(live) = live {
        for k in MACHINE_SHARED_KEYS {
            if let Some(v) = live.get(k) {
                out.insert(k.to_owned(), v.clone());
            }
        }
    }
    Ok(serde_json::to_vec(&Value::Object(out)).expect("a Value always serializes"))
}

pub fn machine_shared_only(live: &Map<String, Value>) -> Map<String, Value> {
    live.iter()
        .filter(|(k, _)| MACHINE_SHARED_KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

pub fn setup_token_credential(token: &str) -> Vec<u8> {
    serde_json::to_vec(
        &json!({"claudeAiOauth": {"accessToken": token, "scopes": ["user:inference"]}}),
    )
    .expect("a Value always serializes")
}

pub fn identity_from_oauth_account(v: &Value) -> Option<Identity> {
    let email = v
        .get("emailAddress")?
        .as_str()
        .filter(|s| !s.is_empty())?
        .to_owned();
    let str_field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    Some(Identity {
        label: email.clone(),
        email: Some(email),
        org_uuid: str_field("organizationUuid").unwrap_or_default(),
        org_name: str_field("organizationName"),
        account_uuid: str_field("accountUuid").filter(|s| !s.is_empty()),
        raw: v.clone(),
    })
}

/// The identity cswap records for token accounts.
///
/// Precondition: `email` must be non-empty; an empty email panics (see
/// [`identity_from_oauth_account`]'s empty-email rejection).
pub fn token_identity(email: &str) -> Identity {
    let raw = json!({"emailAddress": email, "accountUuid": "", "organizationUuid": null, "organizationName": null});
    identity_from_oauth_account(&raw).expect("a non-empty email always parses")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn oauth(rt: &str) -> Vec<u8> {
        json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": rt, "expiresAt": 1, "refreshTokenExpiresAt": 99}})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn classification_follows_section_7_1() {
        assert_eq!(classify(b"sk-ant-api03-abc"), KIND_API_KEY);
        assert_eq!(classify(b"  sk-ant-api03-abc\n"), KIND_API_KEY);
        assert_eq!(classify(&oauth("rt")), KIND_OAUTH);
        assert_eq!(
            classify(&setup_token_credential("sk-ant-oat01-x")),
            KIND_SETUP_TOKEN
        );
    }

    #[test]
    fn fingerprints_cover_every_kind() {
        assert_eq!(
            fingerprint(&oauth("rt-1")),
            Some(Fingerprint::of_secret(b"rt-1"))
        );
        assert_eq!(
            fingerprint(&setup_token_credential("tok")),
            Some(Fingerprint::of_secret(b"tok"))
        );
        assert_eq!(
            fingerprint(b" sk-ant-api03-k\n"),
            Some(Fingerprint::of_secret(b"sk-ant-api03-k"))
        );
        assert_eq!(fingerprint(b"{}"), None);
        assert_eq!(fingerprint(b"garbage"), None);
    }

    #[test]
    fn refresh_wiped_and_expiry() {
        assert!(has_refresh_token(&oauth("rt")));
        assert!(!has_refresh_token(&setup_token_credential("t")));
        let wiped = json!({"claudeAiOauth": {"accessToken": "", "refreshToken": ""}}).to_string();
        assert!(is_wiped(wiped.as_bytes()));
        assert!(!is_wiped(&oauth("rt")));
        assert!(!is_wiped(b"sk-ant-api03-x"));
        assert_eq!(login_expires_at(&oauth("rt")), Some(99));
    }

    #[test]
    fn compose_takes_machine_shared_keys_from_live_absence_included() {
        let target = json!({
            "claudeAiOauth": {"refreshToken": "target"},
            "trustedDeviceToken": "target-device",
            "mcpOAuth": {"stale": true},
            "futureKey": 1
        });
        let live = json!({
            "claudeAiOauth": {"refreshToken": "outgoing"},
            "mcpOAuth": {"current": true},
            "pluginSecrets": {"p": "s"}
        });
        let out = compose(target.to_string().as_bytes(), live.as_object()).unwrap();
        let out: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            out,
            json!({
                "claudeAiOauth": {"refreshToken": "target"},
                "trustedDeviceToken": "target-device",
                "futureKey": 1,
                "mcpOAuth": {"current": true},
                "pluginSecrets": {"p": "s"}
            })
        );
    }

    #[test]
    fn compose_without_live_drops_machine_shared_keys() {
        let target = json!({"claudeAiOauth": {"refreshToken": "t"}, "pluginSecrets": {"old": 1}});
        let out: Value =
            serde_json::from_slice(&compose(target.to_string().as_bytes(), None).unwrap()).unwrap();
        assert_eq!(out, json!({"claudeAiOauth": {"refreshToken": "t"}}));
    }

    #[test]
    fn machine_shared_only_strips_account_keys() {
        let live =
            json!({"claudeAiOauth": {}, "trustedDeviceToken": "d", "x": 1, "mcpOAuth": {"m": 1}});
        assert_eq!(
            Value::Object(machine_shared_only(live.as_object().unwrap())),
            json!({"mcpOAuth": {"m": 1}})
        );
    }

    #[test]
    fn setup_tokens_have_the_spec_shape() {
        assert_eq!(
            setup_token_credential("tok"),
            br#"{"claudeAiOauth":{"accessToken":"tok","scopes":["user:inference"]}}"#.to_vec()
        );
    }

    #[test]
    fn identities_parse_from_oauth_account() {
        let id = identity_from_oauth_account(&json!({
            "emailAddress": "a@b.co", "organizationUuid": null, "organizationName": null, "accountUuid": "u-1"
        }))
        .unwrap();
        assert_eq!(
            (
                id.label.as_str(),
                id.org_uuid.as_str(),
                id.account_uuid.as_deref()
            ),
            ("a@b.co", "", Some("u-1"))
        );
        assert!(identity_from_oauth_account(&json!({"emailAddress": ""})).is_none());
        assert!(identity_from_oauth_account(&json!({})).is_none());
        let t = token_identity("api-key-3@token.local");
        assert_eq!(t.account_uuid, None);
        assert_eq!(
            t.raw,
            json!({"emailAddress": "api-key-3@token.local", "accountUuid": "", "organizationUuid": null, "organizationName": null})
        );
    }

    #[test]
    fn unrecognised_bytes_fall_through_to_setup_token() {
        let wiped = json!({"claudeAiOauth": {"accessToken": "", "refreshToken": ""}})
            .to_string()
            .into_bytes();
        assert_eq!(classify(&wiped), KIND_SETUP_TOKEN);
        assert_eq!(classify(b"{}"), KIND_SETUP_TOKEN);
        assert_eq!(classify(b"garbage"), KIND_SETUP_TOKEN);
        assert_eq!(classify(br#"{"mcpOAuth":{}}"#), KIND_SETUP_TOKEN);
    }

    #[test]
    fn compose_preserves_the_target_s_key_order() {
        let target = json!({
            "claudeAiOauth": {"refreshToken": "t"},
            "mcpOAuth": {},
            "trustedDeviceToken": "d",
            "futureKey": 1
        });
        let out = compose(target.to_string().as_bytes(), json!({}).as_object()).unwrap();
        let out: Value = serde_json::from_slice(&out).unwrap();
        let keys: Vec<&str> = out
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["claudeAiOauth", "trustedDeviceToken", "futureKey"]);
    }

    #[test]
    fn compose_drops_a_target_machine_shared_key_absent_from_live() {
        let target = json!({
            "claudeAiOauth": {"refreshToken": "t"},
            "mcpXaaIdp": {"stale": true}
        });
        let out = compose(target.to_string().as_bytes(), json!({}).as_object()).unwrap();
        let out: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(out, json!({"claudeAiOauth": {"refreshToken": "t"}}));
    }

    #[test]
    fn compose_error_never_quotes_the_credential_bytes() {
        let malformed = b"not json but has sk-ant-secret in it";
        let err = compose(malformed, None).unwrap_err();
        let msg = err.to_string();
        assert!(!msg.contains("sk-ant-secret"), "{msg}");
    }
}
