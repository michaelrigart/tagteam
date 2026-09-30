//! Appendix A.5 and §7.6: the profile request, its reply, and when the provider may ask at all.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::oauth::{PROFILE_TIMEOUT, parse_profile, profile_request};
use tagteam_cc::provider::ClaudeCode;
use tagteam_cc::shape::setup_token_credential;
use tagteam_provider::http::{HttpError, HttpResponse, Method, ScriptedHttp};
use tagteam_provider::{Credential, FakeKeychain, Provider};

/// A recorded reply (Task 1): `{"status", "headers", "body", "synthetic"}`.
fn fixture(raw: &str) -> (HttpResponse, Value) {
    let v: Value = serde_json::from_str(raw).unwrap();
    let body = v["body"].clone();
    let headers = v["headers"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| Some((k.to_lowercase(), v.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let resp = HttpResponse {
        status: v["status"].as_u64().unwrap() as u16,
        headers,
        body: serde_json::to_vec(&body).unwrap(),
    };
    (resp, body)
}

fn reply(status: u16, body: &Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: serde_json::to_vec(body).unwrap(),
    }
}

const NOW: i64 = 1_790_000_000_000;

fn cc() -> ClaudeCode {
    ClaudeCode::with_store(
        LiveStore::new(Arc::new(FakeKeychain::new()), Platform::MacOs)
            .with_retry_delay(Duration::ZERO),
    )
}

fn oauth(expires_at: Value) -> Vec<u8> {
    json!({"claudeAiOauth": {"accessToken": "at-live", "refreshToken": "rt-live", "expiresAt": expires_at}})
        .to_string()
        .into_bytes()
}

#[test]
fn production_and_test_endpoints_follow_appendix_a5() {
    let p = Endpoints::production();
    assert_eq!(p.token, "https://platform.claude.com/v1/oauth/token");
    assert_eq!(p.profile, "https://api.anthropic.com/api/oauth/profile");
    assert_eq!(p.usage, "https://api.anthropic.com/api/oauth/usage");
    let t = Endpoints::with_base("http://127.0.0.1:9/");
    assert_eq!(t.token, "http://127.0.0.1:9/v1/oauth/token");
    assert_eq!(t.profile, "http://127.0.0.1:9/api/oauth/profile");
    assert_eq!(t.usage, "http://127.0.0.1:9/api/oauth/usage");
}

#[test]
fn the_profile_request_is_a_bearer_get_with_a_five_second_timeout() {
    let req = profile_request(&Endpoints::production(), "at-secret");
    assert_eq!(req.method, Method::Get);
    assert_eq!(req.url, "https://api.anthropic.com/api/oauth/profile");
    assert_eq!(req.timeout, Duration::from_secs(5));
    assert_eq!(PROFILE_TIMEOUT, Duration::from_secs(5));
    assert!(req.body.is_none());
    assert!(
        req.headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer at-secret")
    );
    assert!(
        !format!("{req:?}").contains("at-secret"),
        "the bearer never reaches Debug"
    );
}

#[test]
fn the_recorded_profile_reply_resolves_to_its_account() {
    let (resp, body) = fixture(include_str!("fixtures/endpoints/profile-200.json"));
    let id = parse_profile(&resp).expect("the recorded reply resolves");
    assert_eq!(id.account_uuid.as_deref(), body["account"]["uuid"].as_str());
    assert_eq!(id.email.as_deref(), body["account"]["email"].as_str());
    assert_eq!(
        id.org_uuid,
        body["organization"]["uuid"].as_str().unwrap_or_default()
    );
    // `raw` is shaped like CC's `oauthAccount`, so a displaced row records it the same way.
    assert_eq!(id.raw["accountUuid"], body["account"]["uuid"]);
    assert_eq!(id.raw["emailAddress"], body["account"]["email"]);
}

#[test]
fn a_profile_reply_resolves_only_with_a_non_empty_account_uuid() {
    let ok = json!({"account": {"uuid": "u-1", "email": "a@x.co"}, "organization": {"uuid": "o-1", "name": "Org"}});
    let id = parse_profile(&reply(200, &ok)).unwrap();
    assert_eq!(
        (
            id.account_uuid.as_deref(),
            id.email.as_deref(),
            id.org_uuid.as_str(),
            id.org_name.as_deref(),
            id.label.as_str()
        ),
        (Some("u-1"), Some("a@x.co"), "o-1", Some("Org"), "a@x.co")
    );
    let personal = json!({"account": {"uuid": "u-1", "email": "a@x.co"}, "organization": null});
    assert_eq!(parse_profile(&reply(200, &personal)).unwrap().org_uuid, "");
    for body in [
        json!({"account": {"uuid": "", "email": "a@x.co"}}),
        json!({"account": {"email": "a@x.co"}}),
        json!({"account": {"uuid": 7}}),
        json!({}),
    ] {
        assert!(parse_profile(&reply(200, &body)).is_none(), "{body}");
    }
    assert!(
        parse_profile(&reply(401, &ok)).is_none(),
        "only a 200 resolves"
    );
    let garbage = HttpResponse {
        status: 200,
        headers: vec![],
        body: b"<html>captive portal</html>".to_vec(),
    };
    assert!(parse_profile(&garbage).is_none());
}

#[test]
fn resolve_owner_asks_the_profile_endpoint_for_a_live_oauth_token() {
    let http = ScriptedHttp::new();
    let url = Endpoints::production().profile;
    http.push_json(
        Method::Get,
        &url,
        200,
        json!({"account": {"uuid": "u-1", "email": "a@x.co"}, "organization": {"uuid": ""}}),
    );
    let owner = cc().resolve_owner(
        &http,
        &Credential::fresh(oauth(json!(NOW + 3_600_000))),
        NOW,
    );
    assert_eq!(owner.unwrap().account_uuid.as_deref(), Some("u-1"));
    let sent = http.requests();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0]
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer at-live")
    );
}

#[test]
fn resolve_owner_never_asks_without_a_token_it_can_show() {
    // §7.6: an expired access token, a setup token (its only scope is `user:inference`), an API
    // key, or no access token at all: no answer, and no request.
    let http = ScriptedHttp::new();
    let url = Endpoints::production().profile;
    http.push_json(
        Method::Get,
        &url,
        200,
        json!({"account": {"uuid": "u-1", "email": "a@x.co"}}),
    );
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("expired", oauth(json!(NOW + 60_000))), // within §7.2's 5-minute buffer
        ("setup token", setup_token_credential("sk-ant-oat01-setup")),
        ("api key", b"sk-ant-api03-key".to_vec()),
        (
            "no access token",
            json!({"claudeAiOauth": {"refreshToken": "rt"}})
                .to_string()
                .into_bytes(),
        ),
    ];
    for (what, bytes) in cases {
        assert!(
            cc().resolve_owner(&http, &Credential::fresh(bytes), NOW)
                .is_none(),
            "{what}"
        );
    }
    assert_eq!(http.count(Method::Get, &url), 0);
}

#[test]
fn resolve_owner_asks_for_an_oauth_blob_that_has_no_refresh_token() {
    // §7.6 skips only an expired token, a setup token and an API key. A blob with a live access
    // token and profile scopes but no refresh token is none of those, so it is shown.
    let http = ScriptedHttp::new();
    let url = Endpoints::production().profile;
    http.push_json(
        Method::Get,
        &url,
        200,
        json!({"account": {"uuid": "u-1", "email": "a@x.co"}}),
    );
    let blob = json!({"claudeAiOauth": {
        "accessToken": "at-live",
        "expiresAt": NOW + 3_600_000,
        "scopes": ["user:inference", "user:profile"],
    }})
    .to_string()
    .into_bytes();
    let owner = cc().resolve_owner(&http, &Credential::fresh(blob), NOW);
    assert_eq!(owner.unwrap().account_uuid.as_deref(), Some("u-1"));
    assert_eq!(http.count(Method::Get, &url), 1);
}

#[test]
fn a_non_numeric_expiry_counts_as_unexpired() {
    // §7.2: a non-numeric `expiresAt` is not expired, so the token may be shown.
    let http = ScriptedHttp::new();
    let url = Endpoints::production().profile;
    http.push_json(
        Method::Get,
        &url,
        200,
        json!({"account": {"uuid": "u-1", "email": "a@x.co"}}),
    );
    let owner = cc().resolve_owner(&http, &Credential::fresh(oauth(json!("soon"))), NOW);
    assert!(owner.is_some());
}

#[test]
fn a_transport_failure_is_no_answer() {
    let http = ScriptedHttp::new();
    let url = Endpoints::production().profile;
    http.push(
        Method::Get,
        &url,
        Err(HttpError::Ambiguous("timed out".into())),
    );
    assert!(
        cc().resolve_owner(
            &http,
            &Credential::fresh(oauth(json!(NOW + 3_600_000))),
            NOW
        )
        .is_none()
    );
}

#[test]
fn with_endpoints_redirects_every_request() {
    let http = ScriptedHttp::new();
    let base = Endpoints::with_base("http://127.0.0.1:4321");
    http.push_json(
        Method::Get,
        &base.profile,
        200,
        json!({"account": {"uuid": "u-1", "email": "a@x.co"}}),
    );
    let owner = cc().with_endpoints(base.clone()).resolve_owner(
        &http,
        &Credential::fresh(oauth(json!(NOW + 3_600_000))),
        NOW,
    );
    assert!(owner.is_some());
    assert_eq!(http.count(Method::Get, &base.profile), 1);
}
