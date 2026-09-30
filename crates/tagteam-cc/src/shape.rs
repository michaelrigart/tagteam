use serde_json::{Map, Value, json};
use tagteam_core::Fingerprint;
use tagteam_provider::{Identity, KindTraits, ProviderError};

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

/// The stored refresh token, if the credential has one.
pub fn refresh_token(bytes: &[u8]) -> Option<String> {
    oauth_obj(bytes).and_then(|o| token(&o, "refreshToken").map(str::to_owned))
}

/// A refresh reply's token fields (Appendix A.5), ready to apply to the stored credential.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenFields {
    pub access_token: String,
    /// `None` when the reply carried no refresh token: the stored one is kept.
    pub refresh_token: Option<String>,
    pub expires_at: i64,
    /// `None` when the reply carried no `scope`: the stored scopes are kept.
    pub scopes: Option<Vec<String>>,
    pub refresh_token_expires_at: Option<i64>,
}

/// Never shows a token.
impl std::fmt::Debug for TokenFields {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenFields")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .field("refresh_token_expires_at", &self.refresh_token_expires_at)
            .finish_non_exhaustive()
    }
}

/// Appendix A.5: the reply's fields replace the stored ones inside `claudeAiOauth`, the
/// refresh token only when the reply carries one. Every other key, at every level, is kept
/// where it was; new keys are appended.
pub fn apply_refresh(old: &[u8], f: &TokenFields) -> Result<Vec<u8>, ProviderError> {
    let not_an_object =
        || ProviderError::Invalid("the stored credential is not a JSON object".into());
    let mut root = match serde_json::from_slice::<Value>(old) {
        Ok(Value::Object(o)) => o,
        _ => return Err(not_an_object()),
    };
    let entry = root
        .entry("claudeAiOauth")
        .or_insert_with(|| Value::Object(Map::new()));
    let o = entry.as_object_mut().ok_or_else(not_an_object)?;
    o.insert("accessToken".into(), json!(f.access_token));
    if let Some(rt) = &f.refresh_token {
        o.insert("refreshToken".into(), json!(rt));
    }
    o.insert("expiresAt".into(), json!(f.expires_at));
    if let Some(s) = &f.scopes {
        o.insert("scopes".into(), json!(s));
    }
    if let Some(e) = f.refresh_token_expires_at {
        o.insert("refreshTokenExpiresAt".into(), json!(e));
    }
    Ok(serde_json::to_vec(&Value::Object(root)).expect("a Value always serializes"))
}

/// The access token, when the credential has a non-empty one.
pub fn access_token(bytes: &[u8]) -> Option<String> {
    oauth_obj(bytes).and_then(|o| token(&o, "accessToken").map(str::to_owned))
}

/// `claudeAiOauth.expiresAt` in epoch ms; `None` when it is missing or not an integer (§7.2: a
/// non-numeric `expiresAt` counts as not expired).
pub fn access_expires_at(bytes: &[u8]) -> Option<i64> {
    oauth_obj(bytes)?.get("expiresAt")?.as_i64()
}

/// The credential's recorded scopes; empty when it records none.
pub fn scopes(bytes: &[u8]) -> Vec<String> {
    oauth_obj(bytes)
        .and_then(|o| o.get("scopes").and_then(Value::as_array).cloned())
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// §7.2: expired means `now_ms + 5 min >= expiresAt`; an unknown expiry never is.
pub fn is_expired(bytes: &[u8], now_ms: i64) -> bool {
    access_expires_at(bytes).is_some_and(|at| now_ms.saturating_add(300_000) >= at)
}

/// Claude Code's kind traits (§4.5). A kind this provider never stores has none.
pub fn kind_traits(kind: &str) -> KindTraits {
    let plain = KindTraits {
        refreshable: false,
        managed_key_axis: false,
        default_email_prefix: None,
        display: None,
    };
    match kind {
        KIND_OAUTH => KindTraits {
            refreshable: true,
            ..plain
        },
        KIND_SETUP_TOKEN => KindTraits {
            default_email_prefix: Some("setup-token"),
            display: Some("setup token"),
            ..plain
        },
        KIND_API_KEY => KindTraits {
            managed_key_axis: true,
            default_email_prefix: Some("api-key"),
            display: Some("api key"),
            ..plain
        },
        _ => plain,
    }
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

    #[test]
    fn access_token_facts_read_the_access_token() {
        let c = json!({"claudeAiOauth": {
            "accessToken": "at-1", "refreshToken": "rt", "expiresAt": 1_000_000i64,
            "scopes": ["user:inference", "user:profile"]
        }})
        .to_string()
        .into_bytes();
        assert_eq!(access_token(&c).as_deref(), Some("at-1"));
        assert_eq!(access_expires_at(&c), Some(1_000_000));
        assert_eq!(scopes(&c), vec!["user:inference", "user:profile"]);
        // §7.2: expired once `now + 5 min >= expiresAt`.
        assert!(!is_expired(&c, 1_000_000 - 300_001));
        assert!(is_expired(&c, 1_000_000 - 300_000));
    }

    #[test]
    fn a_missing_or_non_numeric_expiry_is_unknown_and_never_expired() {
        for exp in [json!(null), json!("1790000000000"), json!({"at": 1})] {
            let c = json!({"claudeAiOauth": {"accessToken": "at", "expiresAt": exp}})
                .to_string()
                .into_bytes();
            assert_eq!(access_expires_at(&c), None, "{exp}");
            assert!(!is_expired(&c, i64::MAX - 1), "{exp}");
        }
        let bare = json!({"claudeAiOauth": {"accessToken": "at"}})
            .to_string()
            .into_bytes();
        assert_eq!(access_expires_at(&bare), None);
        assert!(scopes(&bare).is_empty());
        assert_eq!(access_token(b"sk-ant-api03-k"), None);
        assert_eq!(access_expires_at(b"sk-ant-api03-k"), None);
        let empty = json!({"claudeAiOauth": {"accessToken": ""}})
            .to_string()
            .into_bytes();
        assert_eq!(access_token(&empty), None, "an empty token is no token");
    }

    #[test]
    fn kind_traits_follow_the_plan_table() {
        let plain = KindTraits {
            refreshable: false,
            managed_key_axis: false,
            default_email_prefix: None,
            display: None,
        };
        assert_eq!(
            kind_traits(KIND_OAUTH),
            KindTraits {
                refreshable: true,
                ..plain
            }
        );
        assert_eq!(
            kind_traits(KIND_SETUP_TOKEN),
            KindTraits {
                default_email_prefix: Some("setup-token"),
                display: Some("setup token"),
                ..plain
            }
        );
        assert_eq!(
            kind_traits(KIND_API_KEY),
            KindTraits {
                managed_key_axis: true,
                default_email_prefix: Some("api-key"),
                display: Some("api key"),
                ..plain
            }
        );
        assert_eq!(kind_traits("not-a-kind"), plain);
    }

    fn fields(rt: Option<&str>) -> TokenFields {
        TokenFields {
            access_token: "at-new".into(),
            refresh_token: rt.map(str::to_owned),
            expires_at: 2_000,
            scopes: Some(vec!["user:inference".into(), "user:profile".into()]),
            refresh_token_expires_at: Some(9_000),
        }
    }

    #[test]
    fn apply_refresh_replaces_the_tokens_and_keeps_every_other_key_in_place() {
        let old = json!({
            "claudeAiOauth": {
                "accessToken": "at-old",
                "refreshToken": "rt-old",
                "expiresAt": 1,
                "scopes": ["user:inference"],
                "subscriptionType": "max",
                "rateLimitTier": "t"
            },
            "trustedDeviceToken": "d",
            "mcpOAuth": {"srv": {"token": "machine-shared"}}
        });
        let out: Value = serde_json::from_slice(
            &apply_refresh(old.to_string().as_bytes(), &fields(Some("rt-new"))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            out,
            json!({
                "claudeAiOauth": {
                    "accessToken": "at-new",
                    "refreshToken": "rt-new",
                    "expiresAt": 2_000,
                    "scopes": ["user:inference", "user:profile"],
                    "subscriptionType": "max",
                    "rateLimitTier": "t",
                    "refreshTokenExpiresAt": 9_000
                },
                "trustedDeviceToken": "d",
                "mcpOAuth": {"srv": {"token": "machine-shared"}}
            })
        );
        let keys: Vec<&str> = out["claudeAiOauth"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "accessToken",
                "refreshToken",
                "expiresAt",
                "scopes",
                "subscriptionType",
                "rateLimitTier",
                "refreshTokenExpiresAt"
            ],
            "existing keys keep their place; new ones are appended"
        );
    }

    #[test]
    fn a_reply_without_a_refresh_token_keeps_the_stored_one() {
        // Review Focus 3: the lineage is unchanged, so the fingerprint is too.
        let old = json!({"claudeAiOauth": {"accessToken": "at-old", "refreshToken": "rt-old", "expiresAt": 1}});
        let out = apply_refresh(old.to_string().as_bytes(), &fields(None)).unwrap();
        assert_eq!(refresh_token(&out).as_deref(), Some("rt-old"));
        assert_eq!(fingerprint(&out), fingerprint(old.to_string().as_bytes()));
        assert_eq!(access_token(&out).as_deref(), Some("at-new"));
    }

    #[test]
    fn apply_refresh_refuses_a_stored_value_that_is_not_an_object() {
        assert!(apply_refresh(b"sk-ant-api03-key", &fields(Some("rt"))).is_err());
        assert!(apply_refresh(br#"{"claudeAiOauth": "x"}"#, &fields(Some("rt"))).is_err());
    }

    #[test]
    fn token_fields_never_print_their_tokens() {
        let shown = format!("{:?}", fields(Some("rt-secret-value")));
        assert!(
            !shown.contains("rt-secret-value") && !shown.contains("at-new"),
            "{shown}"
        );
    }
}
