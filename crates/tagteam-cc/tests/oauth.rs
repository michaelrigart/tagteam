//! Appendix A.5 and §7.6: the profile request, its reply, and when the provider may ask at all.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::endpoints::{CLIENT_ID, Endpoints};
use tagteam_cc::live::{LiveStore, Platform};
use tagteam_cc::oauth::{
    PROFILE_TIMEOUT, TOKEN_TIMEOUT, parse_profile, parse_refresh, profile_request, refresh_request,
};
use tagteam_cc::provider::ClaudeCode;
use tagteam_cc::shape::{self, setup_token_credential};
use tagteam_provider::http::{HttpError, HttpResponse, Method, ScriptedHttp};
use tagteam_provider::provider::{DeadReason, RefreshResult, TransientKind};
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

fn stored() -> Vec<u8> {
    json!({
        "claudeAiOauth": {
            "accessToken": "at-old",
            "refreshToken": "rt-old",
            "expiresAt": 1,
            "scopes": ["user:inference", "user:profile"],
            "subscriptionType": "max"
        },
        "mcpOAuth": {"srv": {"token": "machine-shared"}}
    })
    .to_string()
    .into_bytes()
}

fn successor(r: RefreshResult) -> Value {
    match r {
        RefreshResult::Refreshed { successor, .. } => serde_json::from_slice(&successor).unwrap(),
        other => panic!("expected Refreshed, got {other:?}"),
    }
}

#[test]
fn the_refresh_request_follows_appendix_a5() {
    let scopes = vec!["user:inference".to_owned(), "user:profile".to_owned()];
    let req = refresh_request(
        &Endpoints::production(),
        "rt-secret",
        &scopes,
        TOKEN_TIMEOUT,
    );
    assert_eq!(req.method, Method::Post);
    assert_eq!(req.url, "https://platform.claude.com/v1/oauth/token");
    assert_eq!(req.timeout, Duration::from_secs(10));
    let body: Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"grant_type": "refresh_token", "refresh_token": "rt-secret", "client_id": CLIENT_ID, "scope": "user:inference user:profile"})
    );
    assert_eq!(CLIENT_ID, "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
    assert!(!format!("{req:?}").contains("rt-secret"));
    let bare = refresh_request(&Endpoints::production(), "rt", &[], Duration::from_secs(6));
    let body: Value = serde_json::from_slice(bare.body.as_deref().unwrap()).unwrap();
    assert!(
        body.get("scope").is_none(),
        "no stored scopes: none are invented"
    );
    assert_eq!(bare.timeout, Duration::from_secs(6));
}

#[test]
fn the_synthetic_success_reply_yields_its_successor_and_owner() {
    let (resp, _) = fixture(include_str!("fixtures/endpoints/token-200.json"));
    let r = parse_refresh(&stored(), Ok(resp), NOW);
    let RefreshResult::Refreshed { successor, owner } = r else {
        panic!("expected Refreshed");
    };
    let s: Value = serde_json::from_slice(&successor).unwrap();
    let o = &s["claudeAiOauth"];
    // Task 1's exact values (token-200.json).
    assert_eq!(o["accessToken"], "sk-ant-oat01-synthetic-access-token");
    assert_eq!(o["refreshToken"], "sk-ant-ort01-synthetic-refresh-token");
    assert_eq!(o["expiresAt"], json!(NOW + 28_800_000));
    assert_eq!(o["refreshTokenExpiresAt"], json!(NOW + 7_776_000_000i64));
    assert_eq!(o["scopes"], json!(["user:inference", "user:profile"]));
    assert_eq!(o["subscriptionType"], "max", "untouched keys stay");
    assert_eq!(s["mcpOAuth"], json!({"srv": {"token": "machine-shared"}}));
    let owner = owner.expect("the reply names its account");
    assert_eq!(
        owner.account_uuid.as_deref(),
        Some("00000000-0000-4000-8000-000000000001")
    );
    assert_eq!(owner.email.as_deref(), Some("probe1@example.com"));
    assert_eq!(owner.org_uuid, "00000000-0000-4000-8000-000000000002");
}

#[test]
fn the_recorded_error_replies_are_dead_and_systemic() {
    let (grant, _) = fixture(include_str!("fixtures/endpoints/token-invalid-grant.json"));
    assert!(matches!(
        parse_refresh(&stored(), Ok(grant), NOW),
        RefreshResult::Dead(DeadReason::InvalidGrant)
    ));
    // The real unknown-client answer: a 400 with a nested `invalid_request_error`, not RFC
    // 6749's top-level `invalid_client` (Appendix A.5). The server's message is quoted.
    let (client, body) = fixture(include_str!("fixtures/endpoints/token-invalid-client.json"));
    assert_eq!(client.status, 400);
    assert!(
        body["error"].is_object(),
        "the recording is the nested shape"
    );
    let RefreshResult::Systemic(message) = parse_refresh(&stored(), Ok(client), NOW) else {
        panic!("expected Systemic");
    };
    assert_eq!(message, body["error"]["message"].as_str().unwrap());
    assert!(message.starts_with("Client with id "), "{message}");
}

#[test]
fn both_client_refusal_shapes_are_systemic_and_quote_the_server() {
    let systemic =
        |status: u16, body: Value| match parse_refresh(&stored(), Ok(reply(status, &body)), NOW) {
            RefreshResult::Systemic(m) => m,
            other => panic!("expected Systemic, got {other:?}"),
        };
    // RFC 6749: top-level `invalid_client`, any status; `error_description` when present,
    // else the code.
    for status in [400, 401, 403] {
        assert_eq!(
            systemic(
                status,
                json!({"error": "invalid_client", "error_description": "bad client"})
            ),
            "bad client"
        );
        assert_eq!(
            systemic(status, json!({"error": "invalid_client"})),
            "invalid_client"
        );
    }
    assert_eq!(
        systemic(
            400,
            json!({"error": "invalid_client", "error_description": ""})
        ),
        "invalid_client",
        "an empty description is no message"
    );
    // The endpoint's own shape: a 400 with a nested `invalid_request_error`.
    let nested = |message: Option<&str>| {
        let mut error = json!({"type": "invalid_request_error"});
        if let Some(m) = message {
            error["message"] = json!(m);
        }
        json!({"type": "error", "error": error, "request_id": "req_1"})
    };
    assert_eq!(
        systemic(400, nested(Some("Client with id x not found"))),
        "Client with id x not found"
    );
    assert_eq!(systemic(400, nested(None)), "invalid_request_error");
    // Only a 400 carrying that exact nested type is a client refusal; everything else keeps
    // its old row.
    let transient =
        |status: u16, body: Value| match parse_refresh(&stored(), Ok(reply(status, &body)), NOW) {
            RefreshResult::Transient(k) => k,
            other => panic!("expected Transient, got {other:?}"),
        };
    assert_eq!(transient(401, nested(Some("m"))), TransientKind::Http(401));
    assert_eq!(transient(500, nested(Some("m"))), TransientKind::Http(500));
    assert_eq!(
        transient(
            400,
            json!({"type": "error", "error": {"type": "api_error", "message": "m"}})
        ),
        TransientKind::Http(400),
    );
    assert_eq!(
        transient(400, json!({"error": {"message": "no type"}})),
        TransientKind::Http(400),
    );
    // A nested error is never a strike: it can't be `invalid_grant`.
    assert_eq!(
        transient(400, json!({"error": {"type": "invalid_grant"}})),
        TransientKind::Http(400),
    );
}

#[test]
fn every_row_of_the_verdict_table() {
    // §7.3 step 7, the provider's half (the engine re-reads the lineage before quarantining).
    let err = |e: &str| json!({"error": e});
    for status in [400, 401, 403] {
        assert!(
            matches!(
                parse_refresh(&stored(), Ok(reply(status, &err("invalid_grant"))), NOW),
                RefreshResult::Dead(DeadReason::InvalidGrant)
            ),
            "{status}"
        );
    }
    let transient = |r: RefreshResult| match r {
        RefreshResult::Transient(k) => k,
        other => panic!("expected Transient, got {other:?}"),
    };
    // Only 400/401/403 with `invalid_grant` is a strike.
    assert_eq!(
        transient(parse_refresh(
            &stored(),
            Ok(reply(500, &err("invalid_grant"))),
            NOW
        )),
        TransientKind::Http(500)
    );
    let k = transient(parse_refresh(
        &stored(),
        Ok(reply(401, &err("unauthorized"))),
        NOW,
    ));
    assert_eq!(
        (k.clone(), k.token()),
        (TransientKind::Http(401), "http-401".to_owned())
    );
    assert_eq!(
        transient(parse_refresh(&stored(), Ok(reply(429, &json!({}))), NOW)),
        TransientKind::Http(429)
    );
    for status in [400, 401] {
        // RFC 6749's top-level shape; the nested one is pinned by the recorded fixture and by
        // `both_client_refusal_shapes_are_systemic_and_quote_the_server`.
        assert!(matches!(
            parse_refresh(&stored(), Ok(reply(status, &err("invalid_client"))), NOW),
            RefreshResult::Systemic(m) if m == "invalid_client"
        ));
    }
    let pre = transient(parse_refresh(
        &stored(),
        Err(HttpError::PreSend("dns".into())),
        NOW,
    ));
    assert_eq!(
        (pre.clone(), pre.token()),
        (TransientKind::PreSend, "pre-send".to_owned())
    );
    let amb = transient(parse_refresh(
        &stored(),
        Err(HttpError::Ambiguous("reset".into())),
        NOW,
    ));
    assert_eq!(
        (amb.clone(), amb.token()),
        (TransientKind::Ambiguous, "ambiguous".to_owned())
    );
}

#[test]
fn a_success_reply_with_nothing_usable_is_a_bad_response() {
    // Review Focus 2: no token of either kind, or no JSON at all: nothing was received, so
    // nothing may be persisted, quarantined or rescued.
    let transient = |r: RefreshResult| match r {
        RefreshResult::Transient(k) => k,
        other => panic!("expected Transient, got {other:?}"),
    };
    let empty = transient(parse_refresh(
        &stored(),
        Ok(reply(
            200,
            &json!({"token_type": "Bearer", "expires_in": 28800}),
        )),
        NOW,
    ));
    assert_eq!(
        (empty.clone(), empty.token()),
        (TransientKind::BadResponse, "bad-response".to_owned())
    );
    let html = HttpResponse {
        status: 200,
        headers: vec![],
        body: b"<html>proxy login</html>".to_vec(),
    };
    assert_eq!(
        transient(parse_refresh(&stored(), Ok(html), NOW)),
        TransientKind::BadResponse
    );
}

#[test]
fn a_reply_naming_a_refresh_token_is_never_discarded() {
    // §7.3: a successor tagteam received is persisted, even from an incomplete reply. Without
    // an access token or an expiry, it is stamped expired, so the next use refreshes again.
    let s = successor(parse_refresh(
        &stored(),
        Ok(reply(200, &json!({"refresh_token": "rt-new"}))),
        NOW,
    ));
    assert_eq!(s["claudeAiOauth"]["refreshToken"], "rt-new");
    assert_eq!(s["claudeAiOauth"]["expiresAt"], json!(NOW));
    let s = successor(parse_refresh(
        &stored(),
        Ok(reply(
            200,
            &json!({"access_token": "at-new", "refresh_token": "rt-new"}),
        )),
        NOW,
    ));
    assert_eq!(
        s["claudeAiOauth"]["expiresAt"],
        json!(NOW),
        "no expires_in: expires now"
    );
}

#[test]
fn a_reply_without_a_refresh_token_keeps_the_lineage() {
    // Review Focus 3.
    let s = successor(parse_refresh(
        &stored(),
        Ok(reply(
            200,
            &json!({"access_token": "at-new", "expires_in": 60}),
        )),
        NOW,
    ));
    let bytes = serde_json::to_vec(&s).unwrap();
    assert_eq!(shape::fingerprint(&bytes), shape::fingerprint(&stored()));
    assert_eq!(s["claudeAiOauth"]["accessToken"], "at-new");
    assert_eq!(s["claudeAiOauth"]["expiresAt"], json!(NOW + 60_000));
    assert_eq!(
        s["claudeAiOauth"]["scopes"],
        json!(["user:inference", "user:profile"]),
        "no scope in the reply: the stored scopes stay"
    );
}

#[test]
fn refresh_sends_the_stored_token_and_scopes() {
    let http = ScriptedHttp::new();
    let url = Endpoints::production().token;
    http.push_json(
        Method::Post,
        &url,
        200,
        json!({"access_token": "at-new", "refresh_token": "rt-new", "expires_in": 28800}),
    );
    let cred = Credential::fresh(stored()).into_fresh().unwrap();
    let r = cc().refresh(&http, &cred, NOW, Duration::from_secs(6));
    assert_eq!(successor(r)["claudeAiOauth"]["refreshToken"], "rt-new");
    let sent = http.requests();
    assert_eq!(sent.len(), 1);
    let body: Value = serde_json::from_slice(sent[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(body["refresh_token"], "rt-old");
    assert_eq!(body["scope"], "user:inference user:profile");
}

#[test]
fn a_credential_without_a_refresh_token_is_dead_and_never_sent() {
    let http = ScriptedHttp::new();
    let url = Endpoints::production().token;
    let access_only = Credential::fresh(
        json!({"claudeAiOauth": {"accessToken": "only-access"}})
            .to_string()
            .into_bytes(),
    )
    .into_fresh()
    .unwrap();
    assert!(matches!(
        cc().refresh(&http, &access_only, NOW, TOKEN_TIMEOUT),
        RefreshResult::Dead(DeadReason::NoRefreshToken)
    ));
    let setup = Credential::fresh(setup_token_credential("sk-ant-oat01-x"))
        .into_fresh()
        .unwrap();
    assert!(matches!(
        cc().refresh(&http, &setup, NOW, TOKEN_TIMEOUT),
        RefreshResult::Dead(DeadReason::NoRefreshToken)
    ));
    assert_eq!(http.count(Method::Post, &url), 0);
}

#[test]
fn an_organization_alone_names_the_owner() {
    // §7.4: an organization that disagrees is a conflict even when the reply names no
    // account, so the owner is built from the organization alone.
    let r = parse_refresh(
        &stored(),
        Ok(reply(
            200,
            &json!({"access_token": "at-2", "refresh_token": "rt-2", "expires_in": 60,
                    "organization": {"uuid": "org-other"}}),
        )),
        NOW,
    );
    let RefreshResult::Refreshed { owner, .. } = r else {
        panic!("expected Refreshed");
    };
    let owner = owner.expect("the organization names an owner");
    assert_eq!(
        (owner.account_uuid.as_deref(), owner.org_uuid.as_str()),
        (None, "org-other")
    );
    let r = parse_refresh(
        &stored(),
        Ok(reply(
            200,
            &json!({"access_token": "at-2", "refresh_token": "rt-2", "expires_in": 60}),
        )),
        NOW,
    );
    let RefreshResult::Refreshed { owner, .. } = r else {
        panic!("expected Refreshed");
    };
    assert!(owner.is_none(), "a reply naming neither has no owner");
}

#[test]
fn refresh_results_never_print_a_token() {
    let r = parse_refresh(
        &stored(),
        Ok(reply(
            200,
            &json!({"access_token": "at-SENTINEL", "refresh_token": "rt-SENTINEL", "expires_in": 1}),
        )),
        NOW,
    );
    let shown = format!("{r:?}");
    assert!(!shown.contains("SENTINEL"), "{shown}");
    assert_eq!(DeadReason::InvalidGrant.as_str(), "invalid_grant");
    assert_eq!(DeadReason::NoRefreshToken.as_str(), "no_refresh_token");
}
