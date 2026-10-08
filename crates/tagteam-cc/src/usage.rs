//! §8.1 and §8.2, Claude Code's half: the usage request, what its reply means, the generic
//! windows it normalizes to, and cswap's JSON shape rendered back from them (§13.2). The engine
//! owns the budget, the lease, the token and every write (§8.3).

use std::time::Duration;

use serde_json::{Map, Value, json};
use tagteam_core::pace::Pace;
use tagteam_core::usage::{Window, WindowKind};
use tagteam_provider::http::{HttpError, HttpRequest, HttpResponse};
use tagteam_provider::provider::UsageResult;

use crate::endpoints::Endpoints;

/// The ISO 8601 reader and writer moved to `tagteam-core`, which the engine's export file uses
/// too (§13.3); this provider's callers keep their path.
pub use tagteam_core::time::{format_iso8601, parse_iso8601};

/// §8.1, Appendix A.5.
pub const USAGE_TIMEOUT: Duration = Duration::from_secs(5);

/// The `anthropic-beta` value the usage endpoint requires (§8.1).
pub const USAGE_BETA: &str = "oauth-2025-04-20";

/// Claude Code's window keys (§8.2). A scoped window's key is `scoped:<name>`.
const FIVE_HOUR: &str = "5h";
/// The `Long` window, which consume-first ranks on (`Provider::primary_long_window`).
pub(crate) const SEVEN_DAY: &str = "7d";
const SPEND: &str = "spend";
const SCOPED_PREFIX: &str = "scoped:";

const FIVE_HOUR_S: i64 = 18_000;
const WEEK_S: i64 = 604_800;

/// The largest `exponent` or `decimal_places` an amount may carry; beyond it the amount is
/// nonsense and its window is left out.
const MAX_EXPONENT: i64 = 18;

/// `GET /api/oauth/usage`, with the access token as the bearer and the beta header. The
/// adapter sets `User-Agent` itself (§4.4).
pub fn usage_request(e: &Endpoints, access_token: &str) -> HttpRequest {
    HttpRequest::get(e.usage.clone(), USAGE_TIMEOUT)
        .bearer(access_token)
        .header("anthropic-beta", USAGE_BETA)
}

/// `five_hour` or `seven_day`: an object of `utilization` (a percentage) and `resets_at`.
/// Null, or a null or non-finite `utilization`, leaves the window out; a field of the wrong
/// type is `bad-response`.
fn fixed(v: &Value, key: &str, kind: WindowKind, period_s: i64) -> Result<Option<Window>, ()> {
    if v.is_null() {
        return Ok(None);
    }
    let resets_at = match &v["resets_at"] {
        Value::Null => None,
        Value::String(s) => parse_iso8601(s),
        _ => return Err(()),
    };
    let pct = match &v["utilization"] {
        Value::Null => return Ok(None),
        Value::Number(n) => n.as_f64(),
        _ => return Err(()),
    };
    Ok(pct.map(|pct| Window {
        key: key.to_owned(),
        label: key.to_owned(),
        kind,
        pct,
        resets_at,
        period_s: Some(period_s),
        detail: None,
    }))
}

/// `amount / 10^exponent`; `None` for an exponent outside `0..=MAX_EXPONENT`.
fn scaled(amount: f64, exponent: i64) -> Option<f64> {
    (0..=MAX_EXPONENT)
        .contains(&exponent)
        .then(|| amount / 10f64.powi(exponent as i32))
}

/// An `exponent` or `decimal_places` value: 2 when the field is absent or null (§8.2), the
/// integer when it is one, and `None` for anything else (`"3"`, `3.0`, `true`), which leaves
/// the amount's window out rather than guessing a scale.
fn exponent(v: &Value) -> Option<i64> {
    if v.is_null() { Some(2) } else { v.as_i64() }
}

/// A `{amount_minor, currency, exponent}` amount. The exponent defaults to 2, as
/// `extra_usage`'s `decimal_places` does (§8.2).
fn money(v: &Value) -> Option<f64> {
    scaled(v["amount_minor"].as_f64()?, exponent(&v["exponent"])?)
}

/// The spend window: `pct = used / limit · 100`. A zero limit, or a pct that is not finite,
/// leaves it out (§8.2).
fn spend_window(used: f64, limit: f64, currency: Option<&str>) -> Option<Window> {
    if limit == 0.0 {
        return None;
    }
    let pct = used / limit * 100.0;
    pct.is_finite().then(|| Window {
        key: SPEND.to_owned(),
        label: SPEND.to_owned(),
        kind: WindowKind::Spend,
        pct,
        resets_at: None,
        period_s: None,
        detail: Some(json!({"used": used, "limit": limit, "currency": currency})),
    })
}

/// §8.2's two spend rows: `spend` when it is enabled and both amounts carry `amount_minor`;
/// `extra_usage` only when there is no `spend` object at all.
fn spend(body: &Value) -> Option<Window> {
    let s = &body["spend"];
    if s.is_object() {
        if s["enabled"].as_bool() != Some(true) {
            return None;
        }
        let currency = s["limit"]["currency"]
            .as_str()
            .or_else(|| s["used"]["currency"].as_str());
        return spend_window(money(&s["used"])?, money(&s["limit"])?, currency);
    }
    let e = &body["extra_usage"];
    if e["is_enabled"].as_bool() != Some(true) {
        return None;
    }
    let places = exponent(&e["decimal_places"])?;
    let used = scaled(e["used_credits"].as_f64()?, places)?;
    let limit = scaled(e["monthly_limit"].as_f64()?, places)?;
    spend_window(used, limit, Some(e["currency"].as_str()?))
}

/// Each `limits[]` item with a `scope.model.display_name` and a numeric `percent` (§8.2).
/// Two items naming the same model keep the higher pct: the more constraining reading, and one
/// window per key, as `usage_samples`' primary key needs.
fn scoped(limits: &Value) -> Vec<Window> {
    let mut out: Vec<Window> = Vec::new();
    for item in limits.as_array().into_iter().flatten() {
        let Some(name) = item["scope"]["model"]["display_name"]
            .as_str()
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(pct) = item["percent"].as_f64() else {
            continue;
        };
        let w = Window {
            key: format!("{SCOPED_PREFIX}{name}"),
            label: name.to_owned(),
            kind: WindowKind::Scoped,
            pct,
            resets_at: item["resets_at"].as_str().and_then(parse_iso8601),
            period_s: (item["group"].as_str() == Some("weekly")).then_some(WEEK_S),
            detail: None,
        };
        match out.iter_mut().find(|o| o.key == w.key) {
            Some(o) if o.pct < w.pct => *o = w,
            Some(_) => {}
            None => out.push(w),
        }
    }
    out
}

/// §8.2's table, in the order `5h`, `7d`, `spend`, then the scoped windows as listed. Every
/// other field is ignored. `Err(())` is `bad-response`: the body is not an object, or a key
/// tagteam reads has the wrong type (`five_hour`, `seven_day`, `spend` and `extra_usage` must
/// be objects, `limits` an array, `utilization` a number and `resets_at` a string, each when
/// present and not null). Inside `limits[]`, `spend` and `extra_usage`, a field that is missing
/// or of another type only leaves that window out.
#[allow(clippy::result_unit_err)]
pub fn normalize(body: &Value) -> Result<Vec<Window>, ()> {
    if !body.is_object() {
        return Err(());
    }
    let object_or_null = ["five_hour", "seven_day", "spend", "extra_usage"];
    if object_or_null
        .iter()
        .any(|k| !(body[*k].is_null() || body[*k].is_object()))
        || !(body["limits"].is_null() || body["limits"].is_array())
    {
        return Err(());
    }
    let mut out = Vec::new();
    out.extend(fixed(
        &body["five_hour"],
        FIVE_HOUR,
        WindowKind::Short,
        FIVE_HOUR_S,
    )?);
    out.extend(fixed(
        &body["seven_day"],
        SEVEN_DAY,
        WindowKind::Long,
        WEEK_S,
    )?);
    out.extend(spend(body));
    out.extend(scoped(&body["limits"]));
    Ok(out)
}

/// A window key as `normalize` describes it (§8.2), with `pct` 0 and no reset or detail: `5h`,
/// `7d`, `spend`, or `scoped:<name>` for a non-empty name. A scoped window's period comes from
/// its `limits[]` item's `group`, which the key does not carry, so it is described without
/// one. `None` for any other key.
pub fn describe(key: &str) -> Option<Window> {
    let (label, kind, period_s) = match key {
        FIVE_HOUR => (key, WindowKind::Short, Some(FIVE_HOUR_S)),
        SEVEN_DAY => (key, WindowKind::Long, Some(WEEK_S)),
        SPEND => (key, WindowKind::Spend, None),
        _ => {
            let name = key.strip_prefix(SCOPED_PREFIX).filter(|n| !n.is_empty())?;
            (name, WindowKind::Scoped, None)
        }
    };
    Some(Window {
        key: key.to_owned(),
        label: label.to_owned(),
        kind,
        pct: 0.0,
        resets_at: None,
        period_s,
        detail: None,
    })
}

/// §8.1's verdict on one usage reply: `UsageResult::from_reply` with this module's `normalize`.
/// A body that does not normalize is `bad-response`.
pub fn parse_usage(reply: Result<HttpResponse, HttpError>) -> UsageResult {
    UsageResult::from_reply(reply, |body| normalize(body).ok())
}

/// `pct` and `resetsAt?`, appended to `o`.
fn pct_and_reset(mut o: Map<String, Value>, w: &Window) -> Map<String, Value> {
    o.insert("pct".into(), json!(w.pct));
    if let Some(at) = w.resets_at {
        o.insert("resetsAt".into(), json!(format_iso8601(at)));
    }
    o
}

/// `sevenDay`'s fields: the window's, then whichever pace fields §8.7 produced.
fn paced(o: Map<String, Value>, w: &Window, p: &Pace) -> Value {
    let mut o = pct_and_reset(o, w);
    if let Some(e) = p.expected_pct {
        o.insert("expectedPct".into(), json!(e));
    }
    if let Some(a) = p.ahead {
        o.insert("aheadOfPace".into(), json!(a));
    }
    if let Some(at) = p.exhaustion_at {
        o.insert("projectedExhaustionAt".into(), json!(format_iso8601(at)));
    }
    if let Some(l) = p.will_last_to_reset {
        o.insert("willLastToReset".into(), json!(l));
    }
    if let Some(m) = p.method {
        o.insert("projectionMethod".into(), json!(m.as_str()));
    }
    Value::Object(o)
}

/// `{used, limit, pct, currency, resetsAt?}`, the amounts from the window's `detail`.
fn spend_json(w: &Window) -> Value {
    let d = w.detail.as_ref().unwrap_or(&Value::Null);
    let mut o = Map::new();
    o.insert("used".into(), d["used"].clone());
    o.insert("limit".into(), d["limit"].clone());
    o.insert("pct".into(), json!(w.pct));
    o.insert("currency".into(), d["currency"].clone());
    if let Some(at) = w.resets_at {
        o.insert("resetsAt".into(), json!(format_iso8601(at)));
    }
    Value::Object(o)
}

/// §13.2's cswap shape: `fiveHour {pct, resetsAt?}`, `sevenDay {pct, resetsAt?, expectedPct?,
/// aheadOfPace?, projectedExhaustionAt?, willLastToReset?, projectionMethod?}`, `spend {used,
/// limit, pct, currency, resetsAt?}` and `scoped [{name, …sevenDay's fields}]`. A window the
/// reading does not have is `null` (`scoped` is then `[]`). Times are ISO 8601 UTC; a short
/// window's pace is not part of the shape.
pub fn render(windows: &[(Window, Pace)]) -> Value {
    let find = |key: &str| windows.iter().find(|(w, _)| w.key == key);
    let five_hour = find(FIVE_HOUR).map_or(Value::Null, |(w, _)| {
        Value::Object(pct_and_reset(Map::new(), w))
    });
    let seven_day = find(SEVEN_DAY).map_or(Value::Null, |(w, p)| paced(Map::new(), w, p));
    let spend = find(SPEND).map_or(Value::Null, |(w, _)| spend_json(w));
    let scoped: Vec<Value> = windows
        .iter()
        .filter(|(w, _)| w.kind == WindowKind::Scoped)
        .map(|(w, p)| {
            let mut o = Map::new();
            o.insert("name".into(), json!(w.label));
            paced(o, w, p)
        })
        .collect();
    json!({"fiveHour": five_hour, "sevenDay": seven_day, "spend": spend, "scoped": scoped})
}
