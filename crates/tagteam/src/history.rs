//! `tagteam history` (§13.4): the stored samples as sparklines, burn rates and projections, or
//! raw as CSV and JSON. Reads only.

use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_core::ProjectionMethod;
use tagteam_engine::views::{HistoryView, HistoryWindow};

use crate::render;

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// A projection further out than this is not worth a countdown: it will not run out.
const A_YEAR_S: i64 = 365 * 86_400;
/// The widest sparkline. A longer history is bucketed, and each bucket shows its peak.
const SPARK_WIDTH: usize = 60;

/// `--since`: `<n>d`, `<n>h` or `<n>m`, `n` a positive whole number. Seconds.
pub(crate) fn parse_since(s: &str) -> Option<i64> {
    let unit = match s.as_bytes().last()? {
        b'd' => 86_400,
        b'h' => 3_600,
        b'm' => 60,
        _ => return None,
    };
    let digits = &s[..s.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits
        .parse::<i64>()
        .ok()
        .filter(|n| *n > 0)?
        .checked_mul(unit)
}

/// Eight levels from 0 to 100 %. Bucketing keeps a short spike visible: a bucket shows its
/// peak, never an average.
pub(crate) fn sparkline(pcts: &[f64]) -> String {
    if pcts.is_empty() {
        return String::new();
    }
    pcts.chunks(pcts.len().div_ceil(SPARK_WIDTH))
        .map(|bucket| {
            let peak = bucket.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            SPARK[(peak.clamp(0.0, 100.0) / 100.0 * 7.0).round() as usize]
        })
        .collect()
}

/// §13.4's text: per window, its reading and reset, a sparkline of its samples, and its burn
/// rate with the projection and the method behind it. `window` is the `--window` asked for, if
/// any. `now_s` dates the countdowns.
pub(crate) fn human(view: &HistoryView, window: Option<&str>, since: &str, now_s: i64) -> String {
    let row = &view.account.row;
    if view.windows.is_empty() {
        return match window {
            Some(name) if view.unmatched_window => {
                format!("No window named {name} for {}.\n", render::name(row))
            }
            _ => format!("No usage history for {}.\n", render::name(row)),
        };
    }
    let mut s = format!(
        "{} (position {}), last {since}\n",
        render::name(row),
        row.position
    );
    for w in &view.windows {
        s.push('\n');
        s.push_str(&block(w, now_s));
    }
    s
}

fn block(hw: &HistoryWindow, now_s: i64) -> String {
    let w = &hw.window;
    let mut head = format!("{}  {:.0}%", w.label, w.pct.round());
    if let Some(at) = w.resets_at {
        // As `list` says it: a reset that has passed is `reset`, not a countdown to it.
        let left = render::countdown(at, now_s);
        if at > now_s {
            head.push_str(&format!("  resets in {left}"));
        } else {
            head.push_str(&format!("  {left}"));
        }
    }
    let pcts: Vec<f64> = hw.samples.iter().map(|x| x.pct).collect();
    let samples = match pcts.len() {
        0 => "no samples".to_owned(),
        1 => format!("{}  1 sample", sparkline(&pcts)),
        n => format!("{}  {n} samples", sparkline(&pcts)),
    };
    format!("{head}\n  {samples}\n  {}\n", projection(hw, now_s))
}

/// A rate in points per hour with its sign: one decimal, or two below 0.1 in magnitude, where
/// one would print a quiet window's rate as zero (`+0.03`, not `+0.0`).
fn signed(rate: f64) -> String {
    if rate.abs() < 0.1 {
        format!("{rate:+.2}")
    } else {
        format!("{rate:+.1}")
    }
}

/// The burn rate and where it leads (§8.7): `lasts to reset` when the window will, else when it
/// runs out (or `should have run out` when that time has passed, `won't run out within a year`
/// when it is further off than that), with the method that measured the rate.
fn projection(hw: &HistoryWindow, now_s: i64) -> String {
    let p = &hw.pace;
    let method = p
        .method
        .map_or_else(String::new, |m| format!(" ({})", m.as_str()));
    // An exhausted window ran out when it was read, which may be long ago: no countdown applies.
    if hw.window.pct >= 100.0 {
        return match p.rate_per_hour {
            Some(rate) => format!("{} pts/h · at the limit{method}", signed(rate)),
            None => "at the limit".to_owned(),
        };
    }
    let Some(rate) = p.rate_per_hour else {
        return "no rate yet".to_owned();
    };
    let eta = match (p.will_last_to_reset, p.exhaustion_at) {
        (Some(true), _) => "lasts to reset".to_owned(),
        (_, Some(at)) if at <= now_s => format!(
            "should have run out {} ago",
            render::duration(now_s.saturating_sub(at))
        ),
        (_, Some(at)) if at.saturating_sub(now_s) > A_YEAR_S => {
            "won't run out within a year".to_owned()
        }
        (_, Some(at)) => format!("runs out in {}", render::duration(at - now_s)),
        _ => "no projection".to_owned(),
    };
    format!("{} pts/h · {eta}{method}", signed(rate))
}

/// `--csv`: `window,fetched_at,pct,resets_at`, one row per sample, times in ISO 8601 UTC.
pub(crate) fn csv(view: &HistoryView) -> String {
    let mut s = String::from("window,fetched_at,pct,resets_at\n");
    for hw in &view.windows {
        let key = csv_field(&hw.window.key);
        for x in &hw.samples {
            let resets = x.resets_at.map(format_iso8601).unwrap_or_default();
            s.push_str(&format!(
                "{key},{},{},{resets}\n",
                format_iso8601(x.fetched_at),
                x.pct
            ));
        }
    }
    s
}

/// RFC 4180: a field holding a comma, a quote or a line break is quoted, its quotes doubled.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// `--json`: the shape `history --help` documents.
pub(crate) fn json(view: &HistoryView, provider: &str) -> Value {
    let row = &view.account.row;
    let windows: Vec<Value> = view.windows.iter().map(window_json).collect();
    json!({
        "schemaVersion": 1,
        "provider": provider,
        "account": {"number": row.position, "id": row.id.as_str(), "email": render::email(row)},
        "windows": windows,
    })
}

fn window_json(hw: &HistoryWindow) -> Value {
    let (w, p) = (&hw.window, &hw.pace);
    let samples: Vec<Value> = hw
        .samples
        .iter()
        .map(|x| {
            json!({
                "fetchedAt": format_iso8601(x.fetched_at),
                "pct": x.pct,
                "resetsAt": x.resets_at.map(format_iso8601),
            })
        })
        .collect();
    json!({
        "key": w.key,
        "label": w.label,
        "kind": w.kind.as_str(),
        "pct": w.pct,
        "resetsAt": w.resets_at.map(format_iso8601),
        "samples": samples,
        "ratePerHour": p.rate_per_hour,
        "expectedPct": p.expected_pct,
        "aheadOfPace": p.ahead,
        "projectedExhaustionAt": p.exhaustion_at.map(format_iso8601),
        "willLastToReset": p.will_last_to_reset,
        "projectionMethod": p.method.map(ProjectionMethod::as_str),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tagteam_core::{Pace, ProjectionMethod, Sample, Window, WindowKind};
    use tagteam_engine::views::{AccountView, HistoryView, HistoryWindow, UsageStatus};

    use super::*;
    use crate::render::testutil::{NOW, OAUTH, unread, view as account_view, window};

    const HOUR: i64 = 3_600;

    /// `b@x.co` at position 2, with no usage of its own: a history view reads samples instead.
    fn account() -> AccountView {
        account_view(2, "b@x.co", OAUTH, unread(UsageStatus::Ok, None, None))
    }

    /// Readings of one window instance 6, 4 and 2 hours before `NOW`.
    fn samples(pcts: [f64; 3], resets_in: i64) -> Vec<Sample> {
        [6, 4, 2]
            .into_iter()
            .zip(pcts)
            .map(|(hours, pct)| Sample {
                fetched_at: NOW - hours * HOUR,
                pct,
                resets_at: Some(NOW + resets_in),
            })
            .collect()
    }

    fn history_window(window: Window, samples: Vec<Sample>, pace: Pace) -> HistoryWindow {
        HistoryWindow {
            window,
            samples,
            pace,
        }
    }

    /// 5h with no rate yet; 7d running out 12 hours on by regression; Fable, never sampled,
    /// lasting to its reset at the average pace.
    fn view() -> HistoryView {
        let (r5, r7) = (9_600, 266_400);
        HistoryView {
            account: account(),
            unmatched_window: false,
            windows: vec![
                history_window(
                    window("5h", "5h", WindowKind::Short, 9.0, Some(r5)),
                    samples([5.0, 7.0, 9.0], r5),
                    Pace::default(),
                ),
                history_window(
                    window("7d", "7d", WindowKind::Long, 30.0, Some(r7)),
                    samples([10.0, 20.0, 30.0], r7),
                    Pace {
                        rate_per_hour: Some(5.0),
                        method: Some(ProjectionMethod::Regression),
                        exhaustion_at: Some(NOW + 43_200),
                        will_last_to_reset: Some(false),
                        ..Pace::default()
                    },
                ),
                history_window(
                    window("scoped:Fable", "Fable", WindowKind::Scoped, 2.0, Some(r7)),
                    vec![],
                    Pace {
                        expected_pct: Some(44.0),
                        ahead: Some(false),
                        rate_per_hour: Some(0.3),
                        method: Some(ProjectionMethod::Average),
                        exhaustion_at: Some(NOW + 1_000_000),
                        will_last_to_reset: Some(true),
                    },
                ),
            ],
        }
    }

    #[test]
    fn since_takes_whole_days_hours_or_minutes() {
        assert_eq!(parse_since("7d"), Some(604_800));
        assert_eq!(parse_since("12h"), Some(43_200));
        assert_eq!(parse_since("30m"), Some(1_800));
        for bad in [
            "",
            "d",
            "0d",
            "7",
            "7w",
            "1.5h",
            "+7d",
            " 7d",
            "-1d",
            "999999999999999d",
            "99999999999999999999d",
        ] {
            assert_eq!(parse_since(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_sparkline_scales_zero_to_one_hundred() {
        assert_eq!(sparkline(&[]), "");
        assert_eq!(
            sparkline(&[0.0, 14.3, 28.6, 42.9, 57.1, 71.4, 85.7, 100.0]),
            "▁▂▃▄▅▆▇█"
        );
        assert_eq!(sparkline(&[-5.0, 140.0]), "▁█", "clamped to 0–100");
    }

    #[test]
    fn a_long_history_is_bucketed_by_its_peaks() {
        let spike: Vec<f64> = (0..120).map(|i| if i == 7 { 100.0 } else { 0.0 }).collect();
        let line = sparkline(&spike);
        assert_eq!(line.chars().count(), 60);
        assert_eq!(
            line.chars().nth(3),
            Some('█'),
            "a bucket shows its peak: {line}"
        );
        assert_eq!(sparkline(&[0.0; 61]).chars().count(), 31);
    }

    #[test]
    fn the_text_shows_each_windows_rate_and_projection() {
        assert_eq!(
            human(&view(), None, "7d", NOW),
            concat!(
                "b@x.co (position 2), last 7d\n",
                "\n",
                "5h  9%  resets in 2h40m\n",
                "  ▁▁▂  3 samples\n",
                "  no rate yet\n",
                "\n",
                "7d  30%  resets in 3d02h\n",
                "  ▂▂▃  3 samples\n",
                "  +5.0 pts/h · runs out in 12h00m (regression)\n",
                "\n",
                "Fable  2%  resets in 3d02h\n",
                "  no samples\n",
                "  +0.3 pts/h · lasts to reset (average)\n",
            )
        );
    }

    #[test]
    fn a_projection_lasts_runs_out_or_is_missing() {
        let with = |pace: Pace| {
            let w = window("7d", "7d", WindowKind::Long, 30.0, None);
            projection(&history_window(w, vec![], pace), NOW)
        };
        assert_eq!(with(Pace::default()), "no rate yet");
        let past = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW - 60),
            ..Pace::default()
        };
        assert_eq!(
            with(past),
            "+5.0 pts/h · should have run out 1m ago (regression)"
        );
        let just_past = Pace {
            exhaustion_at: Some(NOW),
            ..past
        };
        assert_eq!(
            with(just_past),
            "+5.0 pts/h · should have run out <1m ago (regression)"
        );
        let soon = Pace {
            exhaustion_at: Some(NOW + 60),
            ..past
        };
        assert_eq!(with(soon), "+5.0 pts/h · runs out in 1m (regression)");
        let flat = Pace {
            rate_per_hour: Some(0.0),
            method: Some(ProjectionMethod::Average),
            ..Pace::default()
        };
        assert_eq!(with(flat), "+0.00 pts/h · no projection (average)");
    }

    #[test]
    fn an_exhausted_window_is_at_the_limit_with_or_without_a_rate() {
        let exhausted = |pace: Pace| {
            let w = window("5h", "5h", WindowKind::Short, 100.0, Some(HOUR));
            projection(&history_window(w, vec![], pace), NOW)
        };
        // Read 100, 100, 100: no slope, no average for a Short window, but exhausted at the read.
        let no_rate = Pace {
            exhaustion_at: Some(NOW - 2 * HOUR),
            ..Pace::default()
        };
        assert_eq!(exhausted(no_rate), "at the limit");
        // With a rate the exhaustion time is the read's, hours ago: no countdown.
        let with_rate = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Average),
            exhaustion_at: Some(NOW - 2 * HOUR),
            will_last_to_reset: Some(false),
            ..Pace::default()
        };
        assert_eq!(exhausted(with_rate), "+5.0 pts/h · at the limit (average)");
    }

    #[test]
    fn a_quiet_windows_rate_keeps_two_decimals() {
        for (rate, text) in [
            (0.03, "+0.03"),
            (-0.05, "-0.05"),
            (0.0, "+0.00"),
            (0.099, "+0.10"),
            (0.1, "+0.1"),
            (5.0, "+5.0"),
            (-1.26, "-1.3"),
        ] {
            assert_eq!(signed(rate), text, "{rate}");
        }
    }

    #[test]
    fn a_projection_beyond_a_year_says_it_will_not_run_out() {
        let with = |pace: Pace, reset: Option<i64>| {
            let w = window("7d", "7d", WindowKind::Long, 30.0, reset);
            projection(&history_window(w, vec![], pace), NOW)
        };
        // A quiet window: 70 points to go at 0.006 an hour is well over a year.
        let tiny = Pace {
            rate_per_hour: Some(0.006),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW + 42_000_000),
            will_last_to_reset: None,
            ..Pace::default()
        };
        assert_eq!(
            with(tiny, None),
            "+0.01 pts/h · won't run out within a year (regression)"
        );
        // With a reset it will not reach, the reset is what counts (§8.7).
        let lasts = Pace {
            will_last_to_reset: Some(true),
            ..tiny
        };
        assert_eq!(
            with(lasts, Some(HOUR)),
            "+0.01 pts/h · lasts to reset (regression)"
        );
        // 70 points at 0.01 an hour is 291 days: a countdown still.
        let slow = Pace {
            rate_per_hour: Some(0.01),
            exhaustion_at: Some(NOW + 25_200_000),
            ..tiny
        };
        assert_eq!(
            with(slow, None),
            "+0.01 pts/h · runs out in 291d16h (regression)"
        );
        // The year itself is still a countdown; a second more is not.
        let year = |s| Pace {
            exhaustion_at: Some(NOW + 365 * 86_400 + s),
            ..tiny
        };
        assert_eq!(
            with(year(0), None),
            "+0.01 pts/h · runs out in 365d00h (regression)"
        );
        assert_eq!(
            with(year(1), None),
            "+0.01 pts/h · won't run out within a year (regression)"
        );
        // A saturated time must not overflow.
        let saturated = Pace {
            exhaustion_at: Some(i64::MAX),
            ..tiny
        };
        assert_eq!(
            with(saturated, None),
            "+0.01 pts/h · won't run out within a year (regression)"
        );
    }

    #[test]
    fn a_reset_that_has_passed_says_reset_not_a_countdown() {
        let at = |resets_in: i64| {
            let mut v = view();
            v.windows[0].window.resets_at = Some(NOW + resets_in);
            human(&v, None, "7d", NOW)
        };
        assert!(at(60).contains("5h  9%  resets in 1m\n"), "{}", at(60));
        for passed in [0, -10, -86_400] {
            let text = at(passed);
            assert!(text.contains("5h  9%  reset\n"), "{passed}: {text}");
            assert!(!text.contains("5h  9%  resets in"), "{passed}: {text}");
        }
    }

    #[test]
    fn an_account_without_windows_says_so() {
        let v = HistoryView {
            account: account(),
            windows: vec![],
            unmatched_window: false,
        };
        assert_eq!(human(&v, None, "7d", NOW), "No usage history for b@x.co.\n");
        // A filter that names nothing of an account with no windows is not the filter's fault.
        assert_eq!(
            human(&v, Some("nope"), "7d", NOW),
            "No usage history for b@x.co.\n"
        );
        assert_eq!(csv(&v), "window,fetched_at,pct,resets_at\n");
    }

    #[test]
    fn a_window_filter_that_matched_nothing_says_which_window() {
        let v = HistoryView {
            account: account(),
            windows: vec![],
            unmatched_window: true,
        };
        assert_eq!(
            human(&v, Some("nope"), "7d", NOW),
            "No window named nope for b@x.co.\n"
        );
        let mut aliased = v.clone();
        aliased.account.row.alias = Some("work".into());
        assert_eq!(
            human(&aliased, Some("nope"), "7d", NOW),
            "No window named nope for work (b@x.co).\n"
        );
    }

    #[test]
    fn csv_is_one_row_per_sample_in_iso_8601() {
        assert_eq!(
            csv(&view()),
            concat!(
                "window,fetched_at,pct,resets_at\n",
                "5h,2026-09-21T08:13:20Z,5,2026-09-21T16:53:20Z\n",
                "5h,2026-09-21T10:13:20Z,7,2026-09-21T16:53:20Z\n",
                "5h,2026-09-21T12:13:20Z,9,2026-09-21T16:53:20Z\n",
                "7d,2026-09-21T08:13:20Z,10,2026-09-24T16:13:20Z\n",
                "7d,2026-09-21T10:13:20Z,20,2026-09-24T16:13:20Z\n",
                "7d,2026-09-21T12:13:20Z,30,2026-09-24T16:13:20Z\n",
            )
        );
    }

    #[test]
    fn a_csv_field_is_quoted_only_when_it_must_be() {
        assert_eq!(csv_field("scoped:Fable"), "scoped:Fable");
        assert_eq!(csv_field("scoped:a,b"), "\"scoped:a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        let unreset = HistoryView {
            unmatched_window: false,
            account: account(),
            windows: vec![history_window(
                window("spend", "spend", WindowKind::Spend, 0.0, None),
                vec![Sample {
                    fetched_at: NOW,
                    pct: 0.0,
                    resets_at: None,
                }],
                Pace::default(),
            )],
        };
        assert_eq!(
            csv(&unreset),
            "window,fetched_at,pct,resets_at\nspend,2026-09-21T14:13:20Z,0,\n"
        );
    }

    #[test]
    fn json_carries_every_sample_and_every_projection_field() {
        let v = json(&view(), "claude-code");
        assert_eq!(v["schemaVersion"], 1);
        assert_eq!(v["provider"], "claude-code");
        assert_eq!(
            v["account"],
            json!({"number": 2, "id": "id-2", "email": "b@x.co"})
        );
        assert_eq!(
            v["windows"][1],
            json!({
                "key": "7d", "label": "7d", "kind": "long", "pct": 30.0,
                "resetsAt": "2026-09-24T16:13:20Z",
                "samples": [
                    {"fetchedAt": "2026-09-21T08:13:20Z", "pct": 10.0, "resetsAt": "2026-09-24T16:13:20Z"},
                    {"fetchedAt": "2026-09-21T10:13:20Z", "pct": 20.0, "resetsAt": "2026-09-24T16:13:20Z"},
                    {"fetchedAt": "2026-09-21T12:13:20Z", "pct": 30.0, "resetsAt": "2026-09-24T16:13:20Z"}
                ],
                "ratePerHour": 5.0, "expectedPct": null, "aheadOfPace": null,
                "projectedExhaustionAt": "2026-09-22T02:13:20Z", "willLastToReset": false,
                "projectionMethod": "regression"
            })
        );
        let fable = &v["windows"][2];
        assert_eq!(
            (
                fable["expectedPct"].clone(),
                fable["aheadOfPace"].clone(),
                fable["projectionMethod"].clone(),
                fable["samples"].clone()
            ),
            (json!(44.0), json!(false), json!("average"), json!([]))
        );
        let short = &v["windows"][0];
        assert_eq!(
            (
                short["ratePerHour"].clone(),
                short["projectionMethod"].clone(),
                short["projectedExhaustionAt"].clone()
            ),
            (Value::Null, Value::Null, Value::Null)
        );
    }
}
