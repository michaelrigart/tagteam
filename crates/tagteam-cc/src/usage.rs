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

/// §8.1, Appendix A.5.
pub const USAGE_TIMEOUT: Duration = Duration::from_secs(5);

/// The `anthropic-beta` value the usage endpoint requires (§8.1).
pub const USAGE_BETA: &str = "oauth-2025-04-20";

/// Claude Code's window keys (§8.2). A scoped window's key is `scoped:<name>`.
const FIVE_HOUR: &str = "5h";
const SEVEN_DAY: &str = "7d";
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

/// The value of `n` ASCII digits, or `None` if any byte is not one.
fn digits(b: &[u8]) -> Option<i64> {
    b.iter().try_fold(0i64, |n, c| {
        c.is_ascii_digit().then(|| n * 10 + i64::from(c - b'0'))
    })
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` after 1970-01-01, as `(year, month, day)` (`civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Epoch seconds from `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` (Decision 5). The fraction
/// is dropped. Anything else, a missing offset or an impossible date included, is `None`.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (digits(&b[0..4])?, digits(&b[5..7])?, digits(&b[8..10])?);
    let (hour, minute, second) = (
        digits(&b[11..13])?,
        digits(&b[14..16])?,
        digits(&b[17..19])?,
    );
    let mut rest = &b[19..];
    if let Some(fraction) = rest.strip_prefix(b".") {
        let n = fraction.iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return None;
        }
        rest = &fraction[n..];
    }
    let offset = match rest {
        b"Z" => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let (h, m) = (digits(&[*h1, *h2])?, digits(&[*m1, *m2])?);
            if h > 23 || m > 59 {
                return None;
            }
            if *sign == b'-' {
                -(h * 3600 + m * 60)
            } else {
                h * 3600 + m * 60
            }
        }
        _ => return None,
    };
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// `YYYY-MM-DDTHH:MM:SSZ` for epoch seconds: how rendered JSON spells a time (§13.2).
pub fn format_iso8601(epoch_s: i64) -> String {
    let (year, month, day) = civil_from_days(epoch_s.div_euclid(86_400));
    let secs = epoch_s.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
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

/// A `{amount_minor, currency, exponent}` amount. The exponent defaults to 2, as
/// `extra_usage`'s `decimal_places` does (§8.2).
fn money(v: &Value) -> Option<f64> {
    scaled(
        v["amount_minor"].as_f64()?,
        v["exponent"].as_i64().unwrap_or(2),
    )
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
    let places = e["decimal_places"].as_i64().unwrap_or(2);
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
