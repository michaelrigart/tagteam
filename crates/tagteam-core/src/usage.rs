//! Generic usage windows (§4.5, §8.2): what a provider's usage response is normalized into,
//! how a window is stored in `usage_state.last_good`, and which windows count for decisions.
//!
//! Pure data and functions; no I/O. Times are epoch seconds.

use serde_json::{Map, Value};

/// What a window measures, which decides how the rest of the system treats it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowKind {
    /// A short rolling window (Claude Code's 5 h).
    Short,
    /// The long window that gates a week or a month (Claude Code's 7 d).
    Long,
    /// Money spent against a limit. Never relevant to a decision.
    Spend,
    /// A window scoped to one model or product (Claude Code's weekly per-model windows).
    Scoped,
}

impl WindowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WindowKind::Short => "short",
            WindowKind::Long => "long",
            WindowKind::Spend => "spend",
            WindowKind::Scoped => "scoped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "short" => Some(WindowKind::Short),
            "long" => Some(WindowKind::Long),
            "spend" => Some(WindowKind::Spend),
            "scoped" => Some(WindowKind::Scoped),
            _ => None,
        }
    }

    /// Whether §8.7's average-pace fallback and `aheadOfPace` cover this kind (when the window
    /// also has a period).
    pub fn has_pace(self) -> bool {
        matches!(self, WindowKind::Long | WindowKind::Scoped)
    }
}

/// One usage window, in the provider-neutral form `usage_state.last_good` stores.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// The provider's window key. Claude Code: `5h`, `7d`, `spend`, `scoped:<name>`.
    pub key: String,
    /// What a person sees. Claude Code: `5h`, `7d`, `spend`, `<name>`.
    pub label: String,
    pub kind: WindowKind,
    /// Percent of the window used. Always finite; above 100 is kept (§8.2).
    pub pct: f64,
    /// When the window resets, epoch seconds.
    pub resets_at: Option<i64>,
    /// The window's length in seconds, when the provider states it.
    pub period_s: Option<i64>,
    /// Provider-owned extras (Claude Code's spend: `{used, limit, currency}`).
    pub detail: Option<Value>,
}

/// `last_good`'s stored form: `[{"key","label","kind","pct","resetsAt"?,"periodS"?,"detail"?}]`.
///
/// A window whose `pct` is not finite is left out: JSON has no such number, it would be written
/// as `null`, and a single `null` `pct` makes [`windows_from_json`] discard the whole reading.
/// Normalizers keep `pct` finite, so this only guards the stored form.
pub fn windows_to_json(windows: &[Window]) -> Value {
    Value::Array(
        windows
            .iter()
            .filter(|w| w.pct.is_finite())
            .map(|w| {
                let mut o = Map::new();
                o.insert("key".into(), Value::from(w.key.as_str()));
                o.insert("label".into(), Value::from(w.label.as_str()));
                o.insert("kind".into(), Value::from(w.kind.as_str()));
                o.insert("pct".into(), Value::from(w.pct));
                if let Some(r) = w.resets_at {
                    o.insert("resetsAt".into(), Value::from(r));
                }
                if let Some(p) = w.period_s {
                    o.insert("periodS".into(), Value::from(p));
                }
                if let Some(d) = &w.detail {
                    o.insert("detail".into(), d.clone());
                }
                Value::Object(o)
            })
            .collect(),
    )
}

/// The inverse of [`windows_to_json`]. `None` when the value is not that shape, or any `pct` is
/// not finite: a corrupt row reads as "no reading", never as an error.
pub fn windows_from_json(v: &Value) -> Option<Vec<Window>> {
    v.as_array()?.iter().map(window_from_json).collect()
}

fn window_from_json(v: &Value) -> Option<Window> {
    let o = v.as_object()?;
    let pct = o.get("pct")?.as_f64().filter(|p| p.is_finite())?;
    Some(Window {
        key: o.get("key")?.as_str()?.to_owned(),
        label: o.get("label")?.as_str()?.to_owned(),
        kind: WindowKind::parse(o.get("kind")?.as_str()?)?,
        pct,
        resets_at: optional_i64(o, "resetsAt")?,
        period_s: optional_i64(o, "periodS")?,
        detail: o.get("detail").filter(|d| !d.is_null()).cloned(),
    })
}

/// `Some(None)`: absent or null. `Some(Some(n))`: an integer. `None`: present but not one.
fn optional_i64(o: &Map<String, Value>, key: &str) -> Option<Option<i64>> {
    match o.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(v) => v.as_i64().map(Some),
    }
}

/// §8.2 relevance: `Short` and `Long` always; `Scoped` when `models` names it (case-insensitive)
/// or contains `all`; `Spend` never.
pub fn is_relevant(w: &Window, models: &[String]) -> bool {
    match w.kind {
        WindowKind::Short | WindowKind::Long => true,
        WindowKind::Spend => false,
        WindowKind::Scoped => {
            let label = w.label.to_lowercase();
            models.iter().any(|m| {
                let m = m.to_lowercase();
                m == "all" || m == label
            })
        }
    }
}

fn relevant<'a>(
    windows: &'a [Window],
    models: &'a [String],
) -> impl Iterator<Item = &'a Window> + 'a {
    windows.iter().filter(move |w| is_relevant(w, models))
}

/// `max(relevant pct)`; `None` when no window is relevant.
pub fn max_relevant_pct(windows: &[Window], models: &[String]) -> Option<f64> {
    relevant(windows, models).map(|w| w.pct).reduce(f64::max)
}

/// §8.2: `100 − max(relevant pct)`. `None` is unknown headroom, which is never auto-skipped.
/// Zero or negative means at the limit.
pub fn headroom(windows: &[Window], models: &[String]) -> Option<f64> {
    max_relevant_pct(windows, models).map(|p| 100.0 - p)
}

/// The earliest `resets_at` among relevant windows, if any has one.
pub fn earliest_relevant_reset(windows: &[Window], models: &[String]) -> Option<i64> {
    relevant(windows, models).filter_map(|w| w.resets_at).min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn win(key: &str, label: &str, kind: WindowKind, pct: f64) -> Window {
        Window {
            key: key.into(),
            label: label.into(),
            kind,
            pct,
            resets_at: None,
            period_s: None,
            detail: None,
        }
    }

    fn models(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn sample_windows() -> Vec<Window> {
        vec![
            Window {
                resets_at: Some(1_893_000_000),
                period_s: Some(18_000),
                ..win("5h", "5h", WindowKind::Short, 9.0)
            },
            Window {
                resets_at: Some(1_893_400_000),
                period_s: Some(604_800),
                ..win("7d", "7d", WindowKind::Long, 77.5)
            },
            Window {
                resets_at: Some(1_893_400_000),
                period_s: Some(604_800),
                ..win("scoped:Fable", "Fable", WindowKind::Scoped, 0.0)
            },
            Window {
                detail: Some(json!({"used": 12.5, "limit": 20.0, "currency": "EUR"})),
                ..win("spend", "spend", WindowKind::Spend, 62.5)
            },
        ]
    }

    #[test]
    fn kind_names_round_trip_and_unknown_names_parse_to_none() {
        for (kind, name) in [
            (WindowKind::Short, "short"),
            (WindowKind::Long, "long"),
            (WindowKind::Spend, "spend"),
            (WindowKind::Scoped, "scoped"),
        ] {
            assert_eq!(kind.as_str(), name);
            assert_eq!(WindowKind::parse(name), Some(kind));
        }
        assert_eq!(WindowKind::parse("Short"), None);
        assert_eq!(WindowKind::parse(""), None);
    }

    #[test]
    fn only_long_and_scoped_windows_have_pace() {
        assert!(!WindowKind::Short.has_pace());
        assert!(WindowKind::Long.has_pace());
        assert!(WindowKind::Scoped.has_pace());
        assert!(!WindowKind::Spend.has_pace());
    }

    #[test]
    fn windows_round_trip_through_json_with_detail() {
        let windows = sample_windows();
        let json = windows_to_json(&windows);
        assert_eq!(windows_from_json(&json), Some(windows));
    }

    #[test]
    fn the_stored_form_is_exactly_the_documented_one() {
        let json = windows_to_json(&sample_windows());
        assert_eq!(
            json,
            json!([
                {"key": "5h", "label": "5h", "kind": "short", "pct": 9.0,
                 "resetsAt": 1_893_000_000i64, "periodS": 18_000i64},
                {"key": "7d", "label": "7d", "kind": "long", "pct": 77.5,
                 "resetsAt": 1_893_400_000i64, "periodS": 604_800i64},
                {"key": "scoped:Fable", "label": "Fable", "kind": "scoped", "pct": 0.0,
                 "resetsAt": 1_893_400_000i64, "periodS": 604_800i64},
                {"key": "spend", "label": "spend", "kind": "spend", "pct": 62.5,
                 "detail": {"used": 12.5, "limit": 20.0, "currency": "EUR"}}
            ])
        );
    }

    #[test]
    fn optional_fields_are_omitted_when_absent() {
        let json = windows_to_json(&[win("5h", "5h", WindowKind::Short, 1.0)]);
        let o = json[0].as_object().unwrap();
        assert!(!o.contains_key("resetsAt"));
        assert!(!o.contains_key("periodS"));
        assert!(!o.contains_key("detail"));
    }

    #[test]
    fn an_empty_list_round_trips() {
        assert_eq!(windows_to_json(&[]), json!([]));
        assert_eq!(windows_from_json(&json!([])), Some(Vec::new()));
    }

    #[test]
    fn null_optional_fields_read_as_absent() {
        let v = json!([{"key": "5h", "label": "5h", "kind": "short", "pct": 3.0,
                        "resetsAt": null, "periodS": null, "detail": null}]);
        assert_eq!(
            windows_from_json(&v),
            Some(vec![win("5h", "5h", WindowKind::Short, 3.0)])
        );
    }

    #[test]
    fn an_integer_pct_reads_as_a_float() {
        let v = json!([{"key": "7d", "label": "7d", "kind": "long", "pct": 77}]);
        assert_eq!(windows_from_json(&v).unwrap()[0].pct, 77.0);
    }

    #[test]
    fn a_corrupt_value_reads_as_no_reading() {
        let good = json!({"key": "5h", "label": "5h", "kind": "short", "pct": 1.0});
        let cases = [
            ("not an array", json!({"key": "5h"})),
            ("a string", json!("5h")),
            ("null", json!(null)),
            ("an item that is not an object", json!([1])),
            (
                "missing key",
                json!([{"label": "5h", "kind": "short", "pct": 1.0}]),
            ),
            (
                "missing label",
                json!([{"key": "5h", "kind": "short", "pct": 1.0}]),
            ),
            (
                "missing kind",
                json!([{"key": "5h", "label": "5h", "pct": 1.0}]),
            ),
            (
                "missing pct",
                json!([{"key": "5h", "label": "5h", "kind": "short"}]),
            ),
            (
                "unknown kind",
                json!([{"key": "5h", "label": "5h", "kind": "monthly", "pct": 1.0}]),
            ),
            (
                "pct as a string",
                json!([{"key": "5h", "label": "5h", "kind": "short", "pct": "1.0"}]),
            ),
            (
                "pct null",
                json!([{"key": "5h", "label": "5h", "kind": "short", "pct": null}]),
            ),
            (
                "resetsAt as a string",
                json!([{"key": "5h", "label": "5h", "kind": "short", "pct": 1.0,
                        "resetsAt": "2030-01-01T00:00:00Z"}]),
            ),
            (
                "periodS as a fraction",
                json!([{"key": "5h", "label": "5h", "kind": "short", "pct": 1.0,
                        "periodS": 1.5}]),
            ),
            (
                "key as a number",
                json!([{"key": 5, "label": "5h", "kind": "short", "pct": 1.0}]),
            ),
            (
                "label as a number",
                json!([{"key": "5h", "label": 5, "kind": "short", "pct": 1.0}]),
            ),
            (
                "kind as a number",
                json!([{"key": "5h", "label": "5h", "kind": 1, "pct": 1.0}]),
            ),
            (
                "resetsAt as a float",
                json!([{"key": "5h", "label": "5h", "kind": "short", "pct": 1.0,
                        "resetsAt": 1.5e9}]),
            ),
            (
                "resetsAt beyond i64::MAX",
                json!([{"key": "5h", "label": "5h", "kind": "short", "pct": 1.0,
                        "resetsAt": 9_223_372_036_854_775_808u64}]),
            ),
        ];
        for (name, v) in cases {
            assert_eq!(windows_from_json(&v), None, "{name}");
        }
        // One bad item poisons the whole list; a good item alone is fine.
        assert_eq!(windows_from_json(&json!([good.clone(), 7])), None);
        assert!(windows_from_json(&json!([good])).is_some());
    }

    #[test]
    fn a_non_finite_pct_is_left_out_of_the_stored_form_and_the_rest_round_trips() {
        let good_a = win("5h", "5h", WindowKind::Short, 9.0);
        let good_b = win("7d", "7d", WindowKind::Long, 40.0);
        let windows = vec![
            good_a.clone(),
            win("scoped:Fable", "Fable", WindowKind::Scoped, f64::NAN),
            win("scoped:Opus", "Opus", WindowKind::Scoped, f64::INFINITY),
            good_b.clone(),
        ];
        let json = windows_to_json(&windows);
        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(windows_from_json(&json), Some(vec![good_a, good_b]));
    }

    #[test]
    fn a_non_finite_pct_reads_as_no_reading() {
        // `1e999` is valid JSON that overflows an f64. Parsing it as a number at all relies on
        // serde_json's `arbitrary_precision` feature (enabled in the workspace manifest), which
        // keeps the digits; `as_f64` then gives infinity, which must read as no reading.
        let v: Value = serde_json::from_str(
            r#"[{"key": "5h", "label": "5h", "kind": "short", "pct": 1e999}]"#,
        )
        .unwrap();
        assert_eq!(windows_from_json(&v), None);
    }

    #[test]
    fn short_and_long_windows_are_always_relevant() {
        assert!(is_relevant(&win("5h", "5h", WindowKind::Short, 1.0), &[]));
        assert!(is_relevant(&win("7d", "7d", WindowKind::Long, 1.0), &[]));
        assert!(is_relevant(
            &win("7d", "7d", WindowKind::Long, 1.0),
            &models(&["Fable"])
        ));
    }

    #[test]
    fn a_spend_window_is_never_relevant() {
        let spend = win("spend", "spend", WindowKind::Spend, 99.0);
        assert!(!is_relevant(&spend, &[]));
        assert!(!is_relevant(&spend, &models(&["all"])));
        assert!(!is_relevant(&spend, &models(&["spend"])));
    }

    #[test]
    fn a_scoped_window_is_relevant_when_named_case_insensitively() {
        let fable = win("scoped:Fable", "Fable", WindowKind::Scoped, 10.0);
        assert!(!is_relevant(&fable, &[]));
        assert!(!is_relevant(&fable, &models(&["Opus"])));
        assert!(is_relevant(&fable, &models(&["Fable"])));
        assert!(is_relevant(&fable, &models(&["fable"])));
        assert!(is_relevant(&fable, &models(&["Opus", "FABLE"])));
    }

    #[test]
    fn all_matches_every_scoped_window_in_any_case() {
        let fable = win("scoped:Fable", "Fable", WindowKind::Scoped, 10.0);
        let opus = win("scoped:Opus", "Opus", WindowKind::Scoped, 10.0);
        for m in ["all", "All", "ALL"] {
            assert!(is_relevant(&fable, &models(&[m])), "{m}");
            assert!(is_relevant(&opus, &models(&[m])), "{m}");
        }
    }

    #[test]
    fn headroom_is_one_hundred_minus_the_highest_relevant_pct() {
        let windows = sample_windows();
        // 5h 9.0 and 7d 77.5 are relevant; Fable (0.0) is not named; spend never counts.
        assert_eq!(max_relevant_pct(&windows, &[]), Some(77.5));
        assert_eq!(headroom(&windows, &[]), Some(22.5));
    }

    #[test]
    fn a_named_scoped_window_can_set_the_headroom() {
        let mut windows = sample_windows();
        windows[2].pct = 91.0;
        assert_eq!(headroom(&windows, &[]), Some(22.5));
        assert_eq!(headroom(&windows, &models(&["fable"])), Some(9.0));
        assert_eq!(max_relevant_pct(&windows, &models(&["all"])), Some(91.0));
    }

    #[test]
    fn a_spend_window_never_sets_the_headroom() {
        let windows = vec![
            win("5h", "5h", WindowKind::Short, 10.0),
            win("spend", "spend", WindowKind::Spend, 100.0),
        ];
        assert_eq!(headroom(&windows, &models(&["all"])), Some(90.0));
    }

    #[test]
    fn headroom_is_unknown_when_no_window_is_relevant() {
        assert_eq!(headroom(&[], &[]), None);
        assert_eq!(max_relevant_pct(&[], &[]), None);
        let only_spend = vec![win("spend", "spend", WindowKind::Spend, 50.0)];
        assert_eq!(headroom(&only_spend, &models(&["all"])), None);
        let only_unnamed = vec![win("scoped:Fable", "Fable", WindowKind::Scoped, 50.0)];
        assert_eq!(headroom(&only_unnamed, &[]), None);
    }

    #[test]
    fn headroom_goes_negative_above_one_hundred() {
        let windows = vec![win("7d", "7d", WindowKind::Long, 104.0)];
        assert_eq!(headroom(&windows, &[]), Some(-4.0));
    }

    #[test]
    fn the_earliest_reset_ignores_spend_and_unnamed_scoped_windows() {
        let mut windows = sample_windows();
        // Spend resets earliest of all, and an unnamed scoped window earlier still.
        windows[3].resets_at = Some(1_000);
        windows[2].resets_at = Some(2_000);
        assert_eq!(
            earliest_relevant_reset(&windows, &[]),
            Some(1_893_000_000),
            "spend and the unnamed scoped window are ignored"
        );
        assert_eq!(
            earliest_relevant_reset(&windows, &models(&["Fable"])),
            Some(2_000)
        );
    }

    #[test]
    fn the_earliest_reset_skips_windows_without_one() {
        let windows = vec![
            win("5h", "5h", WindowKind::Short, 1.0),
            Window {
                resets_at: Some(500),
                ..win("7d", "7d", WindowKind::Long, 1.0)
            },
        ];
        assert_eq!(earliest_relevant_reset(&windows, &[]), Some(500));
        assert_eq!(earliest_relevant_reset(&[], &[]), None);
        assert_eq!(
            earliest_relevant_reset(&[win("5h", "5h", WindowKind::Short, 1.0)], &[]),
            None
        );
    }
}
