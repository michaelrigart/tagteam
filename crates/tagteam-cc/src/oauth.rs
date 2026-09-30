//! Appendix A.5: the requests Claude Code's endpoints take and what their replies mean. The
//! provider builds and parses them; the engine owns every lock, write and verdict (§4.5).

use std::time::Duration;

use serde_json::{Value, json};
use tagteam_provider::Identity;
use tagteam_provider::http::{HttpRequest, HttpResponse};

use crate::endpoints::Endpoints;

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
