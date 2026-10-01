//! §8.1, §8.2 and §13.2 for Claude Code: the usage request, the recorded reply's windows,
//! every normalization rule, the verdicts, and the rendered JSON.

use std::time::Duration;

use serde_json::{Value, json};
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::usage::{
    USAGE_BETA, USAGE_TIMEOUT, format_iso8601, normalize, parse_iso8601, parse_usage, render,
    usage_request,
};
use tagteam_core::pace::{Pace, ProjectionMethod};
use tagteam_core::usage::{Window, WindowKind};
use tagteam_provider::http::{HttpError, HttpResponse, Method};
use tagteam_provider::provider::{TransientKind, UsageResult};

/// 2030-01-04T17:20:00Z: the recorded `five_hour.resets_at`.
const FIVE_H_RESET: i64 = 1_893_777_600;
/// 2030-01-08T00:00:00Z: the recorded `seven_day.resets_at`.
const SEVEN_D_RESET: i64 = 1_894_060_800;
/// 2030-01-08T00:00:01Z: the recorded Fable limit's `resets_at`.
const FABLE_RESET: i64 = 1_894_060_801;

/// The recorded reply's body (`usage-200.json`, Appendix A.5).
fn recorded() -> Value {
    let v: Value = serde_json::from_str(include_str!("fixtures/endpoints/usage-200.json")).unwrap();
    v["body"].clone()
}

fn window(
    key: &str,
    label: &str,
    kind: WindowKind,
    pct: f64,
    resets_at: Option<i64>,
    period_s: Option<i64>,
) -> Window {
    Window {
        key: key.into(),
        label: label.into(),
        kind,
        pct,
        resets_at,
        period_s,
        detail: None,
    }
}

fn recorded_windows() -> Vec<Window> {
    vec![
        window(
            "5h",
            "5h",
            WindowKind::Short,
            9.0,
            Some(FIVE_H_RESET),
            Some(18_000),
        ),
        window(
            "7d",
            "7d",
            WindowKind::Long,
            77.0,
            Some(SEVEN_D_RESET),
            Some(604_800),
        ),
        window(
            "scoped:Fable",
            "Fable",
            WindowKind::Scoped,
            0.0,
            Some(FABLE_RESET),
            Some(604_800),
        ),
    ]
}

fn spend(pct: f64, used: f64, limit: f64, currency: &str) -> Window {
    Window {
        detail: Some(json!({"used": used, "limit": limit, "currency": currency})),
        ..window("spend", "spend", WindowKind::Spend, pct, None, None)
    }
}

fn keys(windows: &[Window]) -> Vec<&str> {
    windows.iter().map(|w| w.key.as_str()).collect()
}

fn reply(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Result<HttpResponse, HttpError> {
    Ok(HttpResponse {
        status,
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        body: body.to_vec(),
    })
}

fn failed(kind: TransientKind, retry_after_s: Option<f64>) -> UsageResult {
    UsageResult::Failed {
        kind,
        retry_after_s,
    }
}

#[test]
fn the_usage_request_follows_section_8_1() {
    let req = usage_request(&Endpoints::production(), "at-secret");
    assert_eq!(req.method, Method::Get);
    assert_eq!(req.url, "https://api.anthropic.com/api/oauth/usage");
    assert_eq!(req.timeout, Duration::from_secs(5));
    assert_eq!(USAGE_TIMEOUT, Duration::from_secs(5));
    assert!(req.body.is_none());
    assert_eq!(USAGE_BETA, "oauth-2025-04-20");
    assert_eq!(
        req.headers,
        vec![
            ("authorization", "Bearer at-secret".to_owned()),
            ("anthropic-beta", "oauth-2025-04-20".to_owned()),
        ],
        "User-Agent is the adapter's (§4.4)"
    );
    assert!(
        !format!("{req:?}").contains("at-secret"),
        "the bearer never reaches Debug"
    );
    let local = usage_request(&Endpoints::with_base("http://127.0.0.1:9"), "t");
    assert_eq!(local.url, "http://127.0.0.1:9/api/oauth/usage");
}

#[test]
fn iso8601_reads_the_forms_the_endpoint_writes_and_nothing_else() {
    for (s, want) in [
        ("2030-01-04T17:20:00.000000+00:00", 1_893_777_600),
        ("2030-01-08T00:00:01+00:00", 1_894_060_801),
        ("2030-01-04T17:20:00Z", 1_893_777_600),
        ("2030-01-04T17:20:00.999Z", 1_893_777_600),
        ("2026-09-30T12:34:56.789-05:30", 1_790_791_496),
        ("2000-03-01T00:00:00+14:00", 951_818_400),
        ("2024-02-29T23:59:59+00:00", 1_709_251_199),
        ("1970-01-01T00:00:00Z", 0),
        ("1969-12-31T23:59:59Z", -1),
    ] {
        assert_eq!(parse_iso8601(s), Some(want), "{s}");
    }
    for s in [
        "2030-01-04T17:20:00",
        "2030-01-04T17:20:00.000000",
        "2030-13-01T00:00:00Z",
        "2030-00-01T00:00:00Z",
        "2030-02-29T00:00:00Z",
        "2030-04-31T00:00:00Z",
        "2030-01-00T00:00:00Z",
        "2030-01-04 17:20:00Z",
        "2030-01-04t17:20:00Z",
        "2030-01-04T17:20:00z",
        "2030-01-04T24:00:00Z",
        "2030-01-04T17:60:00Z",
        "2030-01-04T17:20:60Z",
        "2030-01-04T17:20:00.Z",
        "2030-01-04T17:20:00+0000",
        "2030-01-04T17:20:00+24:00",
        "2030-01-04T17:20:00+05:60",
        "2030-01-04T17:20:00Zjunk",
        "2030-1-04T17:20:00Z",
        "+2030-01-04T17:20:00Z",
        "tomorrow",
        "",
    ] {
        assert_eq!(parse_iso8601(s), None, "{s}");
    }
}

#[test]
fn iso8601_formatting_is_utc_and_round_trips() {
    assert_eq!(format_iso8601(FIVE_H_RESET), "2030-01-04T17:20:00Z");
    assert_eq!(format_iso8601(0), "1970-01-01T00:00:00Z");
    assert_eq!(format_iso8601(-1), "1969-12-31T23:59:59Z");
    assert_eq!(format_iso8601(1_709_251_199), "2024-02-29T23:59:59Z");
    assert_eq!(format_iso8601(253_402_300_799), "9999-12-31T23:59:59Z");
    let mut t = 0;
    while t <= 253_402_300_799 {
        assert_eq!(parse_iso8601(&format_iso8601(t)), Some(t), "{t}");
        t += 7_777_777;
    }
}

#[test]
fn the_recorded_reply_normalizes_to_its_three_windows() {
    // `spend.enabled` is false, and `extra_usage` is not read because a `spend` object exists.
    // The session and weekly_all limits have no model, and the code-named windows and
    // `seven_day_breakdown` are ignored (§8.2).
    assert_eq!(normalize(&recorded()), Ok(recorded_windows()));
}

#[test]
fn missing_null_and_non_finite_sources_leave_their_window_out() {
    let mut body = recorded();
    body["five_hour"] = Value::Null;
    body.as_object_mut().unwrap().shift_remove("limits");
    assert_eq!(keys(&normalize(&body).unwrap()), ["7d"]);

    let mut body = recorded();
    body["seven_day"]["utilization"] = Value::Null;
    assert_eq!(keys(&normalize(&body).unwrap()), ["5h", "scoped:Fable"]);

    // 1e400 parses (arbitrary precision) but is not finite: that window alone is dropped.
    let text = r#"{"five_hour": {"utilization": 1e400, "resets_at": null},
                   "seven_day": {"utilization": 5.5, "resets_at": null},
                   "limits": [{"percent": 1e400, "group": "weekly",
                               "scope": {"model": {"display_name": "Fable"}}}]}"#;
    let body: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        normalize(&body),
        Ok(vec![window(
            "7d",
            "7d",
            WindowKind::Long,
            5.5,
            None,
            Some(604_800)
        )])
    );

    assert_eq!(normalize(&json!({})), Ok(vec![]), "no windows: no usage");
}

#[test]
fn a_pct_above_100_is_kept_and_an_unreadable_reset_drops_only_the_reset() {
    let mut body = recorded();
    body["five_hour"]["utilization"] = json!(104.5);
    body["seven_day"]["resets_at"] = json!("next tuesday");
    let w = normalize(&body).unwrap();
    assert_eq!(w[0].pct, 104.5);
    assert_eq!((w[1].key.as_str(), w[1].resets_at), ("7d", None));
}

#[test]
fn only_limits_with_a_model_name_and_a_numeric_percent_are_scoped_windows() {
    let mut body = recorded();
    body["limits"] = json!([
        {"kind": "weekly_scoped", "group": "weekly", "percent": 12,
         "resets_at": "2030-01-08T00:00:01+00:00", "scope": {"model": {"display_name": "Opus"}}},
        {"kind": "daily_scoped", "group": "daily", "percent": 3.5,
         "scope": {"model": {"display_name": "Sonnet"}}},
        {"percent": 50, "scope": {"model": {"display_name": ""}}},
        {"percent": "50", "scope": {"model": {"display_name": "Haiku"}}},
        {"percent": 50, "scope": {"model": null}},
        {"percent": 50, "scope": null},
        "not an object",
        7
    ]);
    let w = normalize(&body).unwrap();
    assert_eq!(
        w[2..],
        [
            window(
                "scoped:Opus",
                "Opus",
                WindowKind::Scoped,
                12.0,
                Some(FABLE_RESET),
                Some(604_800)
            ),
            window(
                "scoped:Sonnet",
                "Sonnet",
                WindowKind::Scoped,
                3.5,
                None,
                None
            ),
        ]
    );
}

#[test]
fn two_limits_for_one_model_keep_the_higher_pct() {
    let mut body = recorded();
    let item = |pct: f64, surface: &str| {
        json!({"group": "weekly", "percent": pct, "resets_at": null,
               "scope": {"model": {"display_name": "Fable"}, "surface": surface}})
    };
    body["limits"] = json!([item(20.0, "chat"), item(60.0, "code"), item(40.0, "cowork")]);
    let w = normalize(&body).unwrap();
    assert_eq!(keys(&w), ["5h", "7d", "scoped:Fable"]);
    assert_eq!(w[2].pct, 60.0);
}

#[test]
fn an_enabled_spend_object_is_the_spend_window() {
    let mut body = recorded();
    body["spend"]["enabled"] = json!(true);
    body["spend"]["used"]["amount_minor"] = json!(500);
    let w = normalize(&body).unwrap();
    assert_eq!(keys(&w), ["5h", "7d", "spend", "scoped:Fable"]);
    assert_eq!(w[2], spend(25.0, 5.0, 20.0, "EUR"));

    // The exponent defaults to 2; an absurd one leaves the window out.
    let mut no_exp = body.clone();
    no_exp["spend"]["used"]
        .as_object_mut()
        .unwrap()
        .shift_remove("exponent");
    assert_eq!(
        normalize(&no_exp).unwrap()[2],
        spend(25.0, 5.0, 20.0, "EUR")
    );
    let mut used_currency = body.clone();
    used_currency["spend"]["limit"]
        .as_object_mut()
        .unwrap()
        .shift_remove("currency");
    used_currency["spend"]["used"]["currency"] = json!("USD");
    assert_eq!(
        normalize(&used_currency).unwrap()[2],
        spend(25.0, 5.0, 20.0, "USD"),
        "the limit's currency, else the used amount's"
    );
    let mut absurd = body.clone();
    absurd["spend"]["limit"]["exponent"] = json!(400);
    assert_eq!(
        keys(&normalize(&absurd).unwrap()),
        ["5h", "7d", "scoped:Fable"]
    );

    let mut zero = body.clone();
    zero["spend"]["limit"]["amount_minor"] = json!(0);
    assert_eq!(
        keys(&normalize(&zero).unwrap()),
        ["5h", "7d", "scoped:Fable"],
        "a zero limit leaves the window out"
    );
    let mut no_amount = body.clone();
    no_amount["spend"]["used"]["amount_minor"] = Value::Null;
    assert_eq!(
        keys(&normalize(&no_amount).unwrap()),
        ["5h", "7d", "scoped:Fable"]
    );
}

#[test]
fn extra_usage_is_read_only_without_a_spend_object() {
    let mut body = recorded();
    body["extra_usage"]["is_enabled"] = json!(true);
    body["extra_usage"]["used_credits"] = json!(1500.0);
    assert_eq!(
        keys(&normalize(&body).unwrap()),
        ["5h", "7d", "scoped:Fable"],
        "a spend object exists, even disabled"
    );

    body.as_object_mut().unwrap().shift_remove("spend");
    assert_eq!(normalize(&body).unwrap()[2], spend(75.0, 15.0, 20.0, "EUR"));
    let mut null_spend = recorded();
    null_spend["spend"] = Value::Null;
    null_spend["extra_usage"] = body["extra_usage"].clone();
    null_spend["extra_usage"]["decimal_places"] = json!(0);
    assert_eq!(
        normalize(&null_spend).unwrap()[2],
        spend(75.0, 1500.0, 2000.0, "EUR"),
        "a null spend is no spend object; decimal_places scales both amounts"
    );

    for (field, value) in [
        ("is_enabled", json!(false)),
        ("used_credits", Value::Null),
        ("monthly_limit", Value::Null),
        ("currency", Value::Null),
        ("monthly_limit", json!(0)),
    ] {
        let mut b = body.clone();
        b["extra_usage"][field] = value;
        assert_eq!(
            keys(&normalize(&b).unwrap()),
            ["5h", "7d", "scoped:Fable"],
            "{field}"
        );
    }
}

#[test]
fn a_known_key_of_the_wrong_type_is_a_bad_response() {
    assert_eq!(normalize(&json!([])), Err(()));
    assert_eq!(normalize(&json!("usage")), Err(()));
    for (key, value) in [
        ("five_hour", json!("9%")),
        ("seven_day", json!(77)),
        ("limits", json!({"kind": "session"})),
        ("spend", json!(true)),
        ("extra_usage", json!([])),
    ] {
        let mut body = recorded();
        body[key] = value;
        assert_eq!(normalize(&body), Err(()), "{key}");
    }
    let mut body = recorded();
    body["five_hour"]["utilization"] = json!("9.0");
    assert_eq!(normalize(&body), Err(()));
    let mut body = recorded();
    body["seven_day"]["resets_at"] = json!(1_894_060_800);
    assert_eq!(normalize(&body), Err(()));
}

#[test]
fn every_row_of_the_usage_verdict_table() {
    let body = serde_json::to_vec(&recorded()).unwrap();
    assert_eq!(
        parse_usage(reply(200, &[], &body)),
        UsageResult::Windows(recorded_windows())
    );
    assert_eq!(
        parse_usage(reply(200, &[], b"{}")),
        UsageResult::Windows(vec![])
    );
    assert_eq!(
        parse_usage(reply(200, &[], b"<html>captive portal</html>")),
        failed(TransientKind::BadResponse, None)
    );
    assert_eq!(
        parse_usage(reply(200, &[], br#"{"five_hour": "9%"}"#)),
        failed(TransientKind::BadResponse, None)
    );
    assert_eq!(
        parse_usage(reply(401, &[("retry-after", "30")], b"{}")),
        UsageResult::Unauthorized
    );
    assert_eq!(
        parse_usage(reply(429, &[("retry-after", "120")], b"{}")),
        failed(TransientKind::Http(429), Some(120.0))
    );
    assert_eq!(
        parse_usage(reply(429, &[], b"")),
        failed(TransientKind::Http(429), None)
    );
    assert_eq!(
        parse_usage(reply(
            429,
            &[("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT")],
            b""
        )),
        failed(TransientKind::Http(429), None),
        "only the seconds form is read"
    );
    assert_eq!(
        parse_usage(reply(503, &[("retry-after", "30")], b"")),
        failed(TransientKind::Http(503), Some(30.0))
    );
    assert_eq!(
        parse_usage(reply(403, &[], b"{}")),
        failed(TransientKind::Http(403), None)
    );
    assert_eq!(
        parse_usage(Err(HttpError::PreSend("dns".into()))),
        failed(TransientKind::PreSend, None)
    );
    assert_eq!(
        parse_usage(Err(HttpError::Ambiguous("reset".into()))),
        failed(TransientKind::Ambiguous, None)
    );
}

#[test]
fn the_recorded_windows_render_as_cswap_s_shape() {
    let windows: Vec<(Window, Pace)> = recorded_windows()
        .into_iter()
        .map(|w| (w, Pace::default()))
        .collect();
    assert_eq!(
        render(&windows),
        json!({
            "fiveHour": {"pct": 9.0, "resetsAt": "2030-01-04T17:20:00Z"},
            "sevenDay": {"pct": 77.0, "resetsAt": "2030-01-08T00:00:00Z"},
            "spend": null,
            "scoped": [{"name": "Fable", "pct": 0.0, "resetsAt": "2030-01-08T00:00:01Z"}]
        })
    );
    assert_eq!(
        render(&[]),
        json!({"fiveHour": null, "sevenDay": null, "spend": null, "scoped": []})
    );
}

#[test]
fn pace_fields_render_on_seven_day_and_scoped_windows_only() {
    let full = Pace {
        expected_pct: Some(50.0),
        ahead: Some(true),
        rate_per_hour: Some(1.5),
        method: Some(ProjectionMethod::Average),
        exhaustion_at: Some(1_893_900_000),
        will_last_to_reset: Some(false),
    };
    let regression = Pace {
        rate_per_hour: Some(4.0),
        method: Some(ProjectionMethod::Regression),
        exhaustion_at: Some(1_894_000_000),
        will_last_to_reset: Some(true),
        ..Pace::default()
    };
    let w = recorded_windows();
    let windows = vec![
        (w[0].clone(), regression),
        (w[1].clone(), full),
        (spend(25.0, 5.0, 20.0, "EUR"), Pace::default()),
        (w[2].clone(), regression),
    ];
    assert_eq!(
        render(&windows),
        json!({
            "fiveHour": {"pct": 9.0, "resetsAt": "2030-01-04T17:20:00Z"},
            "sevenDay": {
                "pct": 77.0,
                "resetsAt": "2030-01-08T00:00:00Z",
                "expectedPct": 50.0,
                "aheadOfPace": true,
                "projectedExhaustionAt": "2030-01-06T03:20:00Z",
                "willLastToReset": false,
                "projectionMethod": "average"
            },
            "spend": {"used": 5.0, "limit": 20.0, "pct": 25.0, "currency": "EUR"},
            "scoped": [{
                "name": "Fable",
                "pct": 0.0,
                "resetsAt": "2030-01-08T00:00:01Z",
                "projectedExhaustionAt": "2030-01-07T07:06:40Z",
                "willLastToReset": true,
                "projectionMethod": "regression"
            }]
        })
    );
}
