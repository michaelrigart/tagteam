//! Appendix A.5: the requests Claude Code's endpoints take and what their replies mean. The
//! provider builds and parses them; the engine owns every lock, write and verdict (§4.5).

use std::time::Duration;

use serde_json::{Value, json};
use tagteam_provider::Identity;
use tagteam_provider::http::{HttpError, HttpRequest, HttpResponse};
use tagteam_provider::provider::{DeadReason, RefreshResult, TransientKind};

use crate::endpoints::{CLIENT_ID, Endpoints};
use crate::shape::{self, TokenFields};

/// §7.6, Appendix A.5.
pub const PROFILE_TIMEOUT: Duration = Duration::from_secs(5);

/// `GET /api/oauth/profile`, with the access token as the bearer.
pub fn profile_request(e: &Endpoints, access_token: &str) -> HttpRequest {
    HttpRequest::get(e.profile.clone(), PROFILE_TIMEOUT).bearer(access_token)
}

fn non_empty(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}

/// An owner named by an endpoint: resolved only with a non-empty account uuid (§7.6). `raw` is
/// shaped like CC's `oauthAccount`, so a displaced row records it as it records a live login.
pub(crate) fn owner_identity(uuid: &Value, email: &Value, org: &Value) -> Option<Identity> {
    let uuid = non_empty(uuid)?.to_owned();
    let email = non_empty(email).map(str::to_owned);
    let org_uuid = org["uuid"].as_str().unwrap_or_default().to_owned();
    let org_name = org["name"].as_str().map(str::to_owned);
    Some(Identity {
        label: email.clone().unwrap_or_else(|| uuid.clone()),
        raw: json!({
            "emailAddress": email,
            "accountUuid": uuid,
            "organizationUuid": org_uuid,
            "organizationName": org_name,
        }),
        email,
        org_uuid,
        org_name,
        account_uuid: Some(uuid),
    })
}

/// Who a 200 profile reply says owns the token: `account.uuid`, `account.email`,
/// `organization.uuid` (Appendix A.5). Anything else is no answer.
pub fn parse_profile(resp: &HttpResponse) -> Option<Identity> {
    if resp.status != 200 {
        return None;
    }
    let body = resp.json()?;
    owner_identity(
        &body["account"]["uuid"],
        &body["account"]["email"],
        &body["organization"],
    )
}

/// §7.3: the gate's bound on its token request. Active-token refresh passes 6 s instead (§7.5).
pub const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);

/// Appendix A.5's refresh request. `scope` is the stored scopes joined by a space; with none
/// stored, none are invented.
pub fn refresh_request(
    e: &Endpoints,
    refresh_token: &str,
    scopes: &[String],
    timeout: Duration,
) -> HttpRequest {
    let mut body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
    });
    if !scopes.is_empty() {
        body["scope"] = json!(scopes.join(" "));
    }
    HttpRequest::post_json(e.token.clone(), &body, timeout)
}

/// Who a token reply says owns the new token (Appendix A.5): its `account.uuid`,
/// `account.email_address` and `organization.uuid`. Unlike the profile oracle (§7.6), either
/// the account uuid or the organization alone is enough: §7.4 quarantines on an organization
/// that disagrees even when the reply names no account. `None` only when it names neither.
pub(crate) fn token_owner(body: &Value) -> Option<Identity> {
    let uuid = non_empty(&body["account"]["uuid"]).map(str::to_owned);
    let org_uuid = non_empty(&body["organization"]["uuid"]).map(str::to_owned);
    if uuid.is_none() && org_uuid.is_none() {
        return None;
    }
    let email = non_empty(&body["account"]["email_address"]).map(str::to_owned);
    let org_name = non_empty(&body["organization"]["name"]).map(str::to_owned);
    let org_uuid = org_uuid.unwrap_or_default();
    Some(Identity {
        label: email
            .clone()
            .or_else(|| uuid.clone())
            .unwrap_or_else(|| org_uuid.clone()),
        raw: json!({
            "emailAddress": email,
            "accountUuid": uuid,
            "organizationUuid": org_uuid,
            "organizationName": org_name,
        }),
        email,
        org_uuid,
        org_name,
        account_uuid: uuid,
    })
}

/// Whole seconds, whether the reply wrote them as an integer or a float.
fn seconds(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// The server's own words for a refusal: `error_description` (RFC 6749) or the nested
/// `error.message`, else `fallback` (the error code), so the text is never empty.
fn refusal_message(body: Option<&Value>, fallback: &str) -> String {
    body.and_then(|b| {
        non_empty(&b["error_description"]).or_else(|| non_empty(&b["error"]["message"]))
    })
    .unwrap_or(fallback)
    .to_owned()
}

/// §7.3 step 7, the provider's half, and Appendix A.5's reply handling.
///
/// - `invalid_grant` from 400, 401 or 403 is `Dead`; the engine re-reads the source that was
///   sent before it quarantines anything.
/// - A refusal of the request itself is `Systemic`, never a strike, quoting the server's
///   message: a top-level `error == "invalid_client"` on any status but 200 (RFC 6749; a 200
///   is not a refusal, and what it delivers is kept), or a 400
///   whose nested `error.type` is `invalid_request_error` (what the endpoint answers for an
///   unknown client id, Appendix A.5). The message is `error_description` or the nested
///   `error.message`, else the error code.
/// - A 200 that names an access or a refresh token is `Refreshed`. Nothing received is ever
///   discarded: without an access token the stored one is kept, and without `expires_in` the
///   successor is stamped as expiring now, so the next use refreshes again.
/// - Anything else is `Transient`.
pub fn parse_refresh(
    old: &[u8],
    reply: Result<HttpResponse, HttpError>,
    now_ms: i64,
) -> RefreshResult {
    let resp = match reply {
        Ok(r) => r,
        Err(HttpError::PreSend(_)) => return RefreshResult::Transient(TransientKind::PreSend),
        Err(HttpError::Ambiguous(_)) => return RefreshResult::Transient(TransientKind::Ambiguous),
    };
    let body = resp.json();
    let error = body.as_ref().map(|b| &b["error"]);
    let code = error.and_then(Value::as_str);
    let nested_type = error.and_then(|e| e["type"].as_str());
    match (resp.status, code, nested_type) {
        (400 | 401 | 403, Some("invalid_grant"), _) => {
            return RefreshResult::Dead(DeadReason::InvalidGrant);
        }
        (status, Some("invalid_client"), _) if status != 200 => {
            return RefreshResult::Systemic(refusal_message(body.as_ref(), "invalid_client"));
        }
        (400, None, Some("invalid_request_error")) => {
            return RefreshResult::Systemic(refusal_message(
                body.as_ref(),
                "invalid_request_error",
            ));
        }
        (200, ..) => {}
        (status, ..) => return RefreshResult::Transient(TransientKind::Http(status)),
    }
    let Some(body) = body else {
        return RefreshResult::Transient(TransientKind::BadResponse);
    };
    let text = |k: &str| non_empty(&body[k]).map(str::to_owned);
    let access = text("access_token");
    let refresh = text("refresh_token");
    if access.is_none() && refresh.is_none() {
        return RefreshResult::Transient(TransientKind::BadResponse);
    }
    let expires_at = match (&access, seconds(&body["expires_in"])) {
        (Some(_), Some(s)) => now_ms.saturating_add(s.saturating_mul(1000)),
        _ => now_ms,
    };
    let fields = TokenFields {
        access_token: access
            .or_else(|| shape::access_token(old))
            .unwrap_or_default(),
        refresh_token: refresh,
        expires_at,
        scopes: body["scope"].as_str().map(|s| {
            s.split(' ')
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .collect()
        }),
        refresh_token_expires_at: seconds(&body["refresh_token_expires_in"])
            .map(|s| now_ms.saturating_add(s.saturating_mul(1000))),
    };
    // `refresh` only sends a credential that parsed as an object, so the first form always
    // applies. The second keeps a received successor even if that ever changes.
    let successor = shape::apply_refresh(old, &fields)
        .or_else(|_| shape::apply_refresh(b"{}", &fields))
        .expect("an empty object always accepts the token fields");
    let owner = token_owner(&body);
    RefreshResult::Refreshed { successor, owner }
}
