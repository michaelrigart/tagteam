//! The daemon in words (spec §5.1, §7.4, §8): the header's mode, from `status` alone, and the
//! activity line's account of the latest event.

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;
use tagteam_daemon::protocol::{Event, StatusProvider};
use tagteam_engine::words::MISSING;

use crate::state::State;
use crate::view::MODE_TIMER;

/// Spec §5.1, §7.4, §8: the header's mode. In live mode, `● live` and the daemon's state for
/// the provider the header follows (`focus`), from the last `status`; else timer mode's, or why
/// the daemon is not followed: a protocol this TUI does not speak, or the daemon's `stopping`.
pub fn mode(state: &State) -> String {
    let d = &state.daemon;
    if d.live() {
        let words = d.job(&super::focus(state)).map_or_else(
            || "daemon idle".to_owned(),
            |job| job_words(job, state.tokens.tz()),
        );
        return format!("● live · {words}");
    }
    if let Some(server) = d.mismatch() {
        return format!("○ daemon protocol {server} unsupported");
    }
    if let Some(reason) = d.stopping() {
        return format!("○ daemon stopping ({reason})");
    }
    MODE_TIMER.to_owned()
}

/// §7.4's table: a provider job's state, and its keychain backoff's end in `tz`.
pub fn job_words(job: &StatusProvider, tz: &TimeZone) -> String {
    let state = match (job.mode.as_str(), job.waiting_for.as_deref()) {
        ("auto", _) => "auto on".to_owned(),
        ("collect", _) if !job.autoswitch => "auto paused".to_owned(),
        ("collect", _) => "auto off (one account)".to_owned(),
        ("waiting", Some("engine-lock")) => match job.holder_pid {
            Some(pid) => format!("auto in pid {pid}"),
            None => "auto in another process".to_owned(),
        },
        ("waiting", Some("schema")) => "daemon needs restart".to_owned(),
        ("idle", _) => "daemon idle".to_owned(),
        (mode, _) => format!("daemon {mode}"),
    };
    match &job.backoff {
        Some(backoff) => format!(
            "{state} · keychain locked until {}",
            iso_clock(&backoff.until, tz)
        ),
        None => state,
    }
}

/// An ISO 8601 time the daemon wrote, as `HH:MM` in `tz`; `—` when it cannot be read.
fn iso_clock(iso: &str, tz: &TimeZone) -> String {
    iso_ms(iso).map_or_else(|| MISSING.to_owned(), |ms| clock(ms, tz))
}

/// An ISO 8601 time, in epoch ms.
pub(crate) fn iso_ms(iso: &str) -> Option<i64> {
    iso.parse::<Timestamp>().ok().map(|t| t.as_millisecond())
}

/// `ms` as `HH:MM` in `tz`.
pub(crate) fn clock(ms: i64, tz: &TimeZone) -> String {
    Timestamp::from_millisecond(ms).map_or_else(
        |_| MISSING.to_owned(),
        |t| t.to_zoned(tz.clone()).strftime("%H:%M").to_string(),
    )
}

/// The activity line's words for `event` (spec §5.1, §8), or none for the kinds it leaves out:
/// `sleep`, `changed`, `mode`, and a kind this TUI does not know. `poll` is the provider's last
/// `poll`, from whose tick a switch names its window. Times are in `tz`.
pub(crate) fn activity(event: &Event, poll: Option<&Value>, tz: &TimeZone) -> Option<String> {
    let text = match event {
        Event::Auto { kind, raw, .. } => auto(kind, raw, poll, tz)?,
        Event::Usage {
            updated,
            fetch_errors,
            ..
        } => {
            let mut parts = Vec::new();
            if !updated.is_empty() {
                let positions: Vec<String> = updated.iter().map(u32::to_string).collect();
                parts.push(format!("collected {}", positions.join(", ")));
            }
            if !fetch_errors.is_empty() {
                let failed: Vec<String> = fetch_errors
                    .iter()
                    .map(|(position, kind)| format!("{position} ({kind})"))
                    .collect();
                parts.push(format!("not collected: {}", failed.join(", ")));
            }
            if parts.is_empty() {
                parts.push("collected".to_owned());
            }
            parts.join(" · ")
        }
        Event::Stopping { reason } => format!("daemon stopping ({reason})"),
        Event::TokenIndex { .. } => "token index updated".to_owned(),
        Event::Changed | Event::Mode { .. } | Event::Unknown { .. } => return None,
    };
    Some(text)
}

/// One of auto's kinds (§11.4), from its JSON, in auto's own words where it has them.
fn auto(kind: &str, raw: &Value, poll: Option<&Value>, tz: &TimeZone) -> Option<String> {
    let str_of = |field: &str| raw[field].as_str().unwrap_or_default().to_owned();
    Some(match kind {
        "poll" => {
            let active = raw["active"]["number"].as_u64();
            match active {
                Some(n) => match raw["headroomPct"][n.to_string()].as_f64() {
                    Some(headroom) => format!("auto: polled · {n} at {}%", used(headroom)),
                    None => format!("auto: polled · {n}"),
                },
                None => "auto: polled".to_owned(),
            }
        }
        "switch" => {
            let (from, to) = (raw["from"].as_u64()?, raw["to"].as_u64()?);
            let why = poll.and_then(|p| binding(p, from)).map_or_else(
                || str_of("trigger"),
                |(window, pct)| format!("{window} at {pct}%"),
            );
            let verb = if raw["dryRun"].as_bool() == Some(true) {
                "auto would switch"
            } else {
                "auto"
            };
            format!("{verb}: {from} → {to} ({why})")
        }
        "no-switch" => {
            let (reason, detail) = (str_of("reason"), str_of("detail"));
            if detail.is_empty() {
                format!("auto: no switch: {reason}")
            } else {
                format!("auto: no switch: {reason} ({detail})")
            }
        }
        "all-exhausted" => match raw["earliestResetAt"].as_str().and_then(iso_ms) {
            Some(at) => format!(
                "auto: no switch: all-exhausted, the first back at {}",
                clock(at, tz)
            ),
            None => "auto: no switch: all-exhausted".to_owned(),
        },
        "account-quarantined" | "account-unquarantined" => {
            let what = kind.trim_start_matches("account-");
            format!(
                "{} {} {what} ({})",
                raw["number"].as_u64()?,
                str_of("email"),
                str_of("reason")
            )
        }
        "error" => format!("error: {}", str_of("message")),
        "config-warning" => format!("warning: {}", str_of("message")),
        _ => return None,
    })
}

/// A headroom as the usage it leaves, rounded as `list` rounds a percentage.
fn used(headroom: f64) -> i64 {
    (100.0 - headroom).round() as i64
}

/// The window that bound account `position` at `poll`: its relevant usage is 100 less its
/// `headroomPct`, and the binding window is the one at that usage among its `windowsPct` (D6.1
/// picks it the same way).
fn binding(poll: &Value, position: u64) -> Option<(String, i64)> {
    let at = position.to_string();
    let headroom = poll["headroomPct"][&at].as_f64()?;
    let usage = 100.0 - headroom;
    poll["windowsPct"][&at]
        .as_object()?
        .iter()
        .find(|(_, pct)| pct.as_f64().is_some_and(|p| (p - usage).abs() < 0.05))
        .map(|(window, _)| (window.clone(), used(headroom)))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use tagteam_daemon::protocol::StatusBackoff;

    use super::*;
    use crate::testutil::*;

    fn utc() -> TimeZone {
        TimeZone::UTC
    }

    #[test]
    fn each_daemon_state_of_spec_7_4_has_its_words_with_and_without_a_keychain_backoff() {
        for (mode, autoswitch, waiting_for, holder, words) in [
            ("auto", true, None, None, "auto on"),
            ("collect", false, None, None, "auto paused"),
            ("collect", true, None, None, "auto off (one account)"),
            (
                "waiting",
                true,
                Some("engine-lock"),
                Some(4711),
                "auto in pid 4711",
            ),
            (
                "waiting",
                true,
                Some("engine-lock"),
                None,
                "auto in another process",
            ),
            (
                "waiting",
                true,
                Some("schema"),
                None,
                "daemon needs restart",
            ),
            ("idle", true, None, None, "daemon idle"),
            ("added-later", true, None, None, "daemon added-later"),
        ] {
            let mut job = daemon_job("claude-code", mode, autoswitch);
            job.waiting_for = waiting_for.map(str::to_owned);
            job.holder_pid = holder;
            assert_eq!(job_words(&job, &utc()), words, "{mode} {waiting_for:?}");
            job.backoff = Some(StatusBackoff {
                reason: "keychain".into(),
                until: "2026-09-21T15:05:00Z".into(),
            });
            assert_eq!(
                job_words(&job, &utc()),
                format!("{words} · keychain locked until 15:05"),
                "{mode} {waiting_for:?}"
            );
        }
    }

    fn auto_event(kind: &str, fields: Value) -> Event {
        let mut raw = json!({
            "schemaVersion": 1,
            "event": kind,
            "ts": "2026-09-21T14:13:20Z",
            "provider": "claude-code",
        });
        for (k, v) in fields.as_object().unwrap() {
            raw[k] = v.clone();
        }
        Event::Auto {
            kind: kind.into(),
            provider: "claude-code".into(),
            raw,
        }
    }

    #[test]
    fn the_activity_line_says_each_event_in_auto_s_words_and_leaves_out_four_kinds() {
        // Spec §8: every event but `sleep`, `changed` and `mode`; a kind it does not know is
        // ignored.
        let poll = json!({"active": {"number": 1, "email": "a@x.co"}, "headroomPct": {"1": 23.0, "2": 80.0}, "threshold": 90.0});
        let cases = [
            (auto_event("poll", poll), Some("auto: polled · 1 at 77%")),
            (
                auto_event("poll", json!({"active": null, "headroomPct": {}})),
                Some("auto: polled"),
            ),
            (
                auto_event(
                    "switch",
                    json!({"trigger": "proactive", "from": 1, "to": 2, "warnings": [], "dryRun": false}),
                ),
                Some("auto: 1 → 2 (proactive)"),
            ),
            (
                auto_event(
                    "switch",
                    json!({"trigger": "reactive", "from": 2, "to": 1, "warnings": [], "dryRun": true}),
                ),
                Some("auto would switch: 2 → 1 (reactive)"),
            ),
            (
                auto_event(
                    "no-switch",
                    json!({"reason": "below-threshold", "detail": ""}),
                ),
                Some("auto: no switch: below-threshold"),
            ),
            (
                auto_event(
                    "no-switch",
                    json!({"reason": "cooldown", "detail": "4m left"}),
                ),
                Some("auto: no switch: cooldown (4m left)"),
            ),
            (
                auto_event(
                    "all-exhausted",
                    json!({"earliestResetAt": "2026-09-21T16:40:00Z"}),
                ),
                Some("auto: no switch: all-exhausted, the first back at 16:40"),
            ),
            (
                auto_event("all-exhausted", json!({"earliestResetAt": null})),
                Some("auto: no switch: all-exhausted"),
            ),
            (
                auto_event(
                    "account-quarantined",
                    json!({"number": 2, "email": "b@x.co", "reason": "invalid_grant"}),
                ),
                Some("2 b@x.co quarantined (invalid_grant)"),
            ),
            (
                auto_event(
                    "account-unquarantined",
                    json!({"number": 2, "email": "b@x.co", "reason": "relogin"}),
                ),
                Some("2 b@x.co unquarantined (relogin)"),
            ),
            (
                auto_event(
                    "error",
                    json!({"message": "the store is locked", "transient": true}),
                ),
                Some("error: the store is locked"),
            ),
            (
                auto_event("config-warning", json!({"message": "unknown key x"})),
                Some("warning: unknown key x"),
            ),
            (
                Event::Usage {
                    provider: "claude-code".into(),
                    updated: vec![1, 2],
                    fetch_errors: BTreeMap::from([(3, "http-429".to_owned())]),
                },
                Some("collected 1, 2 · not collected: 3 (http-429)"),
            ),
            (
                Event::Usage {
                    provider: "claude-code".into(),
                    updated: vec![],
                    fetch_errors: BTreeMap::from([(3, "http-429".to_owned())]),
                },
                Some("not collected: 3 (http-429)"),
            ),
            (
                Event::Stopping {
                    reason: "upgrade".into(),
                },
                Some("daemon stopping (upgrade)"),
            ),
            (
                Event::TokenIndex {
                    provider: "claude-code".into(),
                    raw: json!({}),
                },
                Some("token index updated"),
            ),
            (
                auto_event("sleep", json!({"seconds": 300.0, "until": "x"})),
                None,
            ),
            (Event::Changed, None),
            (
                Event::Mode {
                    provider: "claude-code".into(),
                    mode: "auto".into(),
                    waiting_for: None,
                    holder_pid: None,
                    backoff: None,
                },
                None,
            ),
            (
                Event::Unknown {
                    kind: "added-later".into(),
                    raw: json!({}),
                },
                None,
            ),
        ];
        for (event, words) in cases {
            assert_eq!(
                activity(&event, None, &utc()).as_deref(),
                words,
                "{event:?}"
            );
        }
    }

    #[test]
    fn a_switch_names_the_window_that_bound_it_at_its_tick_s_poll() {
        // Spec §5.1's `auto: 1 → 2 (5h at 95%)`: the binding window is the one at the usage
        // `headroomPct` leaves, as D6.1 finds it.
        let poll = json!({
            "headroomPct": {"1": 5.0},
            "windowsPct": {"1": {"7d": 40.0, "5h": 95.0, "7d_opus": 98.0}},
        });
        let switch = auto_event(
            "switch",
            json!({"trigger": "proactive", "from": 1, "to": 2, "warnings": [], "dryRun": false}),
        );
        assert_eq!(
            activity(&switch, Some(&poll), &utc()).as_deref(),
            Some("auto: 1 → 2 (5h at 95%)"),
            "not the scoped window the relevant ones leave out"
        );
        let other = json!({"headroomPct": {"2": 5.0}, "windowsPct": {"2": {"5h": 95.0}}});
        assert_eq!(
            activity(&switch, Some(&other), &utc()).as_deref(),
            Some("auto: 1 → 2 (proactive)"),
            "a poll that did not read the account switched from"
        );
    }

    #[test]
    fn an_unreadable_time_is_a_dash() {
        assert_eq!(iso_clock("soon", &utc()), MISSING);
        assert_eq!(clock(NOW_MS, &utc()), "14:13");
    }
}
