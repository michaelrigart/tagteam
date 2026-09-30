//! `FakeAgent`'s network half: its own endpoints, shapes and verdicts, deliberately unlike
//! Claude Code's (§15.2).

use std::time::Duration;

use serde_json::{Value, json};
use tagteam_fake::{FakeAgent, credential_json, identity_json};
use tagteam_provider::http::{Method, ScriptedHttp};
use tagteam_provider::provider::{DeadReason, RefreshResult};
use tagteam_provider::{Credential, Provider};

const NOW: i64 = 1_790_000_000_000;

fn token(expires: Option<i64>) -> Credential {
    Credential::fresh(
        credential_json("fa-tok", Some("fa-renew"), expires)
            .to_string()
            .into_bytes(),
    )
}

#[test]
fn resolve_owner_asks_whoami_with_the_bearer_token() {
    let fa = FakeAgent::new();
    let http = ScriptedHttp::new();
    http.push_json(
        Method::Get,
        &fa.whoami_url(),
        200,
        json!({"uid": "fa-u1", "handle": "neo", "workspace": "zion"}),
    );
    let owner = fa
        .resolve_owner(&http, &token(Some(NOW + 3_600_000)), NOW)
        .unwrap();
    assert_eq!(owner.account_uuid.as_deref(), Some("fa-u1"));
    assert_eq!(owner.email, None, "FakeAgent identities have no email");
    assert_eq!(owner.label, "neo@zion");
    assert_eq!(owner.org_uuid, "zion");
    assert_eq!(owner.raw, identity_json("neo", "zion", "fa-u1"));
    let sent = http.requests();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0]
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("authorization") && v == "Bearer fa-tok")
    );
}

#[test]
fn resolve_owner_never_asks_for_an_expired_or_static_credential() {
    let fa = FakeAgent::new();
    let http = ScriptedHttp::new();
    http.push_json(
        Method::Get,
        &fa.whoami_url(),
        200,
        json!({"uid": "fa-u1", "handle": "neo", "workspace": "zion"}),
    );
    assert!(
        fa.resolve_owner(&http, &token(Some(NOW + 1_000)), NOW)
            .is_none()
    );
    let static_cred = Credential::fresh(
        credential_json("fa-static-tok", None, None)
            .to_string()
            .into_bytes(),
    );
    assert!(fa.resolve_owner(&http, &static_cred, NOW).is_none());
    assert_eq!(http.count(Method::Get, &fa.whoami_url()), 0);
}

#[test]
fn a_whoami_reply_without_a_uid_resolves_nothing() {
    let fa = FakeAgent::new();
    let http = ScriptedHttp::new();
    http.push_json(
        Method::Get,
        &fa.whoami_url(),
        200,
        json!({"uid": "", "handle": "neo", "workspace": "zion"}),
    );
    assert!(fa.resolve_owner(&http, &token(None), NOW).is_none());
}

fn renewable() -> tagteam_provider::FreshCredential {
    token(Some(NOW - 1)).into_fresh().unwrap()
}

#[test]
fn refresh_posts_the_renew_token_and_keeps_the_machine_shared_key() {
    let fa = FakeAgent::new();
    let http = ScriptedHttp::new();
    http.push_json(
        Method::Post,
        &fa.renew_url(),
        200,
        json!({"token": "fa-tok2", "renew": "fa-renew2", "expires_in": 60, "owner": {"uid": "fa-u1", "workspace": "zion"}}),
    );
    let r = fa.refresh(&http, &renewable(), NOW, Duration::from_secs(10));
    let RefreshResult::Refreshed { successor, owner } = r else {
        panic!("expected Refreshed, got {r:?}");
    };
    let s: Value = serde_json::from_slice(&successor).unwrap();
    assert_eq!(s["fa"]["token"], "fa-tok2");
    assert_eq!(s["fa"]["renew"], "fa-renew2");
    assert_eq!(s["fa"]["expires"], json!(NOW + 60_000));
    assert_eq!(s["device"], json!({"id": "machine-shared"}));
    let owner = owner.unwrap();
    assert_eq!(
        (owner.account_uuid.as_deref(), owner.org_uuid.as_str()),
        (Some("fa-u1"), "zion")
    );
    let body: Value = serde_json::from_slice(http.requests()[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(body, json!({"renew": "fa-renew"}));
}

#[test]
fn fake_agent_verdicts() {
    let fa = FakeAgent::new();
    let http = ScriptedHttp::new();
    http.push_json(
        Method::Post,
        &fa.renew_url(),
        401,
        json!({"error": "invalid_grant"}),
    );
    assert!(matches!(
        fa.refresh(&http, &renewable(), NOW, Duration::from_secs(10)),
        RefreshResult::Dead(DeadReason::InvalidGrant)
    ));
    let static_cred = Credential::fresh(
        credential_json("fa-static-tok", None, None)
            .to_string()
            .into_bytes(),
    )
    .into_fresh()
    .unwrap();
    let before = http.count(Method::Post, &fa.renew_url());
    assert!(matches!(
        fa.refresh(&http, &static_cred, NOW, Duration::from_secs(10)),
        RefreshResult::Dead(DeadReason::NoRefreshToken)
    ));
    assert_eq!(
        http.count(Method::Post, &fa.renew_url()),
        before,
        "nothing sent"
    );
}
