//! FakeAgent's usage endpoint, deliberately unlike Claude Code's (§15.2): its meters report a
//! fraction of 1 rather than a percentage, under ids that are not Claude Code's window keys,
//! with different periods, and it renders its own JSON.

use std::time::Duration;

use serde_json::{Map, Value, json};
use tagteam_core::pace::Pace;
use tagteam_core::usage::{Window, WindowKind};
use tagteam_provider::http::{HttpError, HttpRequest, HttpResponse};
use tagteam_provider::provider::UsageResult;

const USAGE_TIMEOUT: Duration = Duration::from_secs(5);
const DAY_S: i64 = 86_400;
const MONTH_S: i64 = 2_592_000;

/// `GET <url>` with the token under FakeAgent's own scheme. It rides in `authorization`, so
/// `HttpRequest`'s `Debug` redacts it like any token (§4.4).
pub(crate) fn usage_request(url: String, token: &str) -> HttpRequest {
    HttpRequest::get(url, USAGE_TIMEOUT).header("authorization", format!("Fake {token}"))
}

/// A meter id as FakeAgent's window: `daily` is a Short window of a day and `monthly` a Long one
/// of 30 days, each with `pct` 0 and no reset. `None` for any other id.
pub(crate) fn describe(id: &str) -> Option<Window> {
    let (kind, period_s) = match id {
        "daily" => (WindowKind::Short, DAY_S),
        "monthly" => (WindowKind::Long, MONTH_S),
        _ => return None,
    };
    Some(Window {
        key: id.to_owned(),
        label: id.to_owned(),
        kind,
        pct: 0.0,
        resets_at: None,
        period_s: Some(period_s),
        detail: None,
    })
}

/// `{"meters": [{"id", "used", "renews"}]}`: `used` is a fraction of 1 and `renews` epoch
/// seconds. A meter `describe` does not know, or one without a finite `used`, is ignored.
/// `None` without a `meters` array.
fn normalize(body: &Value) -> Option<Vec<Window>> {
    let mut out = Vec::new();
    for m in body.get("meters")?.as_array()? {
        let Some(meter) = m["id"].as_str().and_then(describe) else {
            continue;
        };
        let Some(pct) = m["used"]
            .as_f64()
            .map(|used| used * 100.0)
            .filter(|p| p.is_finite())
        else {
            continue;
        };
        out.push(Window {
            pct,
            resets_at: m["renews"].as_i64(),
            ..meter
        });
    }
    Some(out)
}

/// The same verdicts as Claude Code's (§8.1): the engine reads only `UsageResult`.
pub(crate) fn parse_usage(reply: Result<HttpResponse, HttpError>) -> UsageResult {
    UsageResult::from_reply(reply, normalize)
}

/// `{"meters": {"<key>": <pct>}}`. FakeAgent's JSON carries no pace.
pub(crate) fn render(windows: &[(Window, Pace)]) -> Value {
    let meters: Map<String, Value> = windows
        .iter()
        .map(|(w, _)| (w.key.clone(), json!(w.pct)))
        .collect();
    json!({ "meters": meters })
}
