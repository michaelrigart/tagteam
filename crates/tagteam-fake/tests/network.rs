//! `FakeAgent`'s network half: its own endpoints, shapes and verdicts, deliberately unlike
//! Claude Code's (§15.2).

use serde_json::json;
use tagteam_fake::{FakeAgent, credential_json, identity_json};
use tagteam_provider::http::{Method, ScriptedHttp};
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
