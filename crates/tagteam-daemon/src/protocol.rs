//! D5's wire format. Every event is one JSON object in §11.4's envelope, and `tagteam auto
//! --json`, `tagteam daemon --json` and the socket all write it from here (D3), so they never
//! differ. Plan B adds the requests, the replies and the greeting.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_core::time::format_iso8601;
use tagteam_engine::auto::AutoEvent;

use crate::events::DaemonEvent;

/// D5.6: the protocol's version. New ops, kinds and fields are additive and keep it.
pub const PROTOCOL: u32 = 1;

/// One event in §11.4's shape: the envelope `{"schemaVersion":1,"event":<kind>,"ts":"…Z"}`,
/// the additive `provider` (§13.2), then the kind's fields in cswap's spelling. Accounts are
/// keyed by position; times are ISO 8601 UTC. `fetchErrors` and `windowsPct` appear only when
/// they hold something.
pub fn event_json(e: &AutoEvent, now_s: i64) -> Value {
    let (kind, provider, fields) = match e {
        AutoEvent::Poll {
            provider,
            active,
            headroom_pct,
            threshold,
            fetch_errors,
            windows_pct,
        } => {
            let mut f = json!({
                "active": active.as_ref().map(|(number, email)| json!({"number": number, "email": email})),
                "headroomPct": by_position(headroom_pct, |h| json!(h)),
                "threshold": threshold,
            });
            if !fetch_errors.is_empty() {
                f["fetchErrors"] = by_position(fetch_errors, |kind| json!(kind));
            }
            if !windows_pct.is_empty() {
                f["windowsPct"] = by_position(windows_pct, |w| json!(w));
            }
            ("poll", provider, f)
        }
        AutoEvent::Switch {
            provider,
            trigger,
            from,
            to,
            warnings,
            dry_run,
        } => (
            "switch",
            provider,
            json!({"trigger": trigger.as_str(), "from": from, "to": to, "warnings": warnings, "dryRun": dry_run}),
        ),
        AutoEvent::NoSwitch {
            provider,
            reason,
            detail,
        } => (
            "no-switch",
            provider,
            json!({"reason": reason, "detail": detail}),
        ),
        AutoEvent::AccountQuarantined {
            provider,
            number,
            email,
            reason,
        } => (
            "account-quarantined",
            provider,
            json!({"number": number, "email": email, "reason": reason}),
        ),
        AutoEvent::AccountUnquarantined {
            provider,
            number,
            email,
            reason,
        } => (
            "account-unquarantined",
            provider,
            json!({"number": number, "email": email, "reason": reason}),
        ),
        AutoEvent::AllExhausted {
            provider,
            earliest_reset_at,
        } => (
            "all-exhausted",
            provider,
            json!({"earliestResetAt": earliest_reset_at.map(format_iso8601)}),
        ),
        AutoEvent::Sleep {
            provider,
            seconds,
            until,
        } => (
            "sleep",
            provider,
            json!({"seconds": (seconds * 10.0).round() / 10.0, "until": format_iso8601(*until)}),
        ),
        AutoEvent::Error {
            provider,
            message,
            transient,
        } => (
            "error",
            provider,
            json!({"message": message, "transient": transient}),
        ),
        AutoEvent::ConfigWarning { provider, message } => {
            ("config-warning", provider, json!({"message": message}))
        }
    };
    let mut o = json!({
        "schemaVersion": 1,
        "event": kind,
        "ts": format_iso8601(now_s),
        "provider": provider.as_str(),
    });
    if let Value::Object(fields) = fields {
        for (k, v) in fields {
            o[k] = v;
        }
    }
    o
}

/// A map keyed by account position, as §11.4's objects key accounts.
fn by_position<T>(m: &BTreeMap<u32, T>, value: impl Fn(&T) -> Value) -> Value {
    Value::Object(
        m.iter()
            .map(|(position, v)| (position.to_string(), value(v)))
            .collect(),
    )
}

/// One event as `subscribe` delivers it (D5.5): an auto kind exactly as `event_json` writes it,
/// or a daemon kind in the same envelope. A daemon kind's optional field is left out, never
/// null, when it holds nothing. `changed` and `stopping` concern no one provider, so they carry
/// no `provider`.
pub fn daemon_event_json(e: &DaemonEvent, now_s: i64) -> Value {
    let (kind, provider, fields) = match e {
        DaemonEvent::Auto(e) => return event_json(e, now_s),
        DaemonEvent::Usage {
            provider,
            updated,
            fetch_errors,
        } => {
            let mut f = json!({"updated": updated});
            if !fetch_errors.is_empty() {
                f["fetchErrors"] = by_position(fetch_errors, |kind| json!(kind));
            }
            ("usage", Some(provider), f)
        }
        DaemonEvent::Changed => ("changed", None, json!({})),
        DaemonEvent::Mode {
            provider,
            mode,
            waiting_for,
            holder_pid,
            backoff,
        } => {
            let mut f = json!({"mode": mode.as_str()});
            if let Some(waiting_for) = waiting_for {
                f["waitingFor"] = json!(waiting_for.as_str());
            }
            if let Some(pid) = holder_pid {
                f["holderPid"] = json!(pid);
            }
            if let Some(backoff) = backoff {
                f["backoff"] =
                    json!({"reason": "keychain", "until": format_iso8601(backoff.until)});
            }
            ("mode", Some(provider), f)
        }
        DaemonEvent::Stopping { reason } => ("stopping", None, json!({"reason": reason.as_str()})),
    };
    envelope(kind, provider, fields, now_s)
}

/// §11.4's envelope around a daemon kind's `fields`, with `provider` when it concerns one.
fn envelope(kind: &str, provider: Option<&ProviderId>, fields: Value, now_s: i64) -> Value {
    let mut o = json!({
        "schemaVersion": 1,
        "event": kind,
        "ts": format_iso8601(now_s),
    });
    if let Some(provider) = provider {
        o["provider"] = json!(provider.as_str());
    }
    if let Value::Object(fields) = fields {
        for (k, v) in fields {
            o[k] = v;
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use tagteam_core::CLAUDE_CODE;
    use tagteam_core::autoswitch::Trigger;

    use super::*;
    use crate::events::{Backoff, Mode, StopReason, WaitingFor};

    /// The engine tests' fixture time, epoch seconds: 2026-09-21T14:13:20Z.
    const T0: i64 = 1_790_000_000;

    fn cc() -> ProviderId {
        ProviderId::new(CLAUDE_CODE)
    }

    fn no_switch_event(reason: &str, detail: &str) -> AutoEvent {
        AutoEvent::NoSwitch {
            provider: cc(),
            reason: reason.into(),
            detail: detail.into(),
        }
    }

    #[test]
    fn every_event_is_one_object_in_section_11_4_s_shape() {
        let poll = AutoEvent::Poll {
            provider: cc(),
            active: Some((2, "b@x.co".into())),
            headroom_pct: BTreeMap::from([(1, Some(80.0)), (2, None)]),
            threshold: 90.0,
            fetch_errors: BTreeMap::from([(2, "http-429".into())]),
            windows_pct: BTreeMap::from([(1, BTreeMap::from([("5h".into(), 20.0)]))]),
        };
        let quiet = AutoEvent::Poll {
            provider: cc(),
            active: None,
            headroom_pct: BTreeMap::new(),
            threshold: 90.0,
            fetch_errors: BTreeMap::new(),
            windows_pct: BTreeMap::new(),
        };
        let cases = [
            (
                poll,
                json!({"event": "poll", "active": {"number": 2, "email": "b@x.co"},
                       "headroomPct": {"1": 80.0, "2": null}, "threshold": 90.0,
                       "fetchErrors": {"2": "http-429"}, "windowsPct": {"1": {"5h": 20.0}}}),
            ),
            (
                quiet,
                json!({"event": "poll", "active": null, "headroomPct": {}, "threshold": 90.0}),
            ),
            (
                AutoEvent::Switch {
                    provider: cc(),
                    trigger: tagteam_core::autoswitch::Trigger::Failover,
                    from: 2,
                    to: 1,
                    warnings: vec!["w".into()],
                    dry_run: true,
                },
                json!({"event": "switch", "trigger": "failover", "from": 2, "to": 1,
                       "warnings": ["w"], "dryRun": true}),
            ),
            (
                no_switch_event("cooldown", "3m"),
                json!({"event": "no-switch", "reason": "cooldown", "detail": "3m"}),
            ),
            (
                AutoEvent::AccountQuarantined {
                    provider: cc(),
                    number: 3,
                    email: "w@corp.com".into(),
                    reason: "invalid_grant".into(),
                },
                json!({"event": "account-quarantined", "number": 3, "email": "w@corp.com",
                       "reason": "invalid_grant"}),
            ),
            (
                AutoEvent::AccountUnquarantined {
                    provider: cc(),
                    number: 3,
                    email: "w@corp.com".into(),
                    reason: "account-replaced".into(),
                },
                json!({"event": "account-unquarantined", "number": 3, "email": "w@corp.com",
                       "reason": "account-replaced"}),
            ),
            (
                AutoEvent::AllExhausted {
                    provider: cc(),
                    earliest_reset_at: Some(T0 + 9_630),
                },
                json!({"event": "all-exhausted", "earliestResetAt": "2026-09-21T16:53:50Z"}),
            ),
            (
                AutoEvent::AllExhausted {
                    provider: cc(),
                    earliest_reset_at: None,
                },
                json!({"event": "all-exhausted", "earliestResetAt": null}),
            ),
            (
                AutoEvent::Sleep {
                    provider: cc(),
                    seconds: 600.0,
                    until: T0 + 600,
                },
                json!({"event": "sleep", "seconds": 600.0, "until": "2026-09-21T14:23:20Z"}),
            ),
            (
                AutoEvent::Error {
                    provider: cc(),
                    message: "m".into(),
                    transient: true,
                },
                json!({"event": "error", "message": "m", "transient": true}),
            ),
            (
                AutoEvent::ConfigWarning {
                    provider: cc(),
                    message: "m".into(),
                },
                json!({"event": "config-warning", "message": "m"}),
            ),
        ];
        for (event, fields) in cases {
            let mut want = json!({"schemaVersion": 1, "event": fields["event"],
                                  "ts": "2026-09-21T14:13:20Z", "provider": "claude-code"});
            for (k, v) in fields.as_object().unwrap() {
                want[k] = v.clone();
            }
            assert_eq!(event_json(&event, T0), want);
        }
        let line = event_json(&no_switch_event("cooldown", ""), T0).to_string();
        assert_eq!(
            line,
            r#"{"schemaVersion":1,"event":"no-switch","ts":"2026-09-21T14:13:20Z","provider":"claude-code","reason":"cooldown","detail":""}"#
        );
    }

    /// Each auto kind, and the line `tagteam auto --json` printed for it before the move: the
    /// same bytes, key order included (D3).
    fn auto_lines() -> Vec<(AutoEvent, &'static str)> {
        vec![
            (
                AutoEvent::Poll {
                    provider: cc(),
                    active: Some((2, "b@x.co".into())),
                    headroom_pct: BTreeMap::from([(1, Some(80.0)), (2, None)]),
                    threshold: 90.0,
                    fetch_errors: BTreeMap::from([(2, "http-429".into())]),
                    windows_pct: BTreeMap::from([(1, BTreeMap::from([("5h".into(), 20.0)]))]),
                },
                r#"{"schemaVersion":1,"event":"poll","ts":"2026-09-21T14:13:20Z","provider":"claude-code","active":{"number":2,"email":"b@x.co"},"headroomPct":{"1":80.0,"2":null},"threshold":90.0,"fetchErrors":{"2":"http-429"},"windowsPct":{"1":{"5h":20.0}}}"#,
            ),
            (
                AutoEvent::Poll {
                    provider: cc(),
                    active: None,
                    headroom_pct: BTreeMap::new(),
                    threshold: 90.0,
                    fetch_errors: BTreeMap::new(),
                    windows_pct: BTreeMap::new(),
                },
                r#"{"schemaVersion":1,"event":"poll","ts":"2026-09-21T14:13:20Z","provider":"claude-code","active":null,"headroomPct":{},"threshold":90.0}"#,
            ),
            (
                AutoEvent::Switch {
                    provider: cc(),
                    trigger: Trigger::Failover,
                    from: 2,
                    to: 1,
                    warnings: vec!["w".into()],
                    dry_run: true,
                },
                r#"{"schemaVersion":1,"event":"switch","ts":"2026-09-21T14:13:20Z","provider":"claude-code","trigger":"failover","from":2,"to":1,"warnings":["w"],"dryRun":true}"#,
            ),
            (
                no_switch_event("cooldown", "3m"),
                r#"{"schemaVersion":1,"event":"no-switch","ts":"2026-09-21T14:13:20Z","provider":"claude-code","reason":"cooldown","detail":"3m"}"#,
            ),
            (
                AutoEvent::AccountQuarantined {
                    provider: cc(),
                    number: 3,
                    email: "w@corp.com".into(),
                    reason: "invalid_grant".into(),
                },
                r#"{"schemaVersion":1,"event":"account-quarantined","ts":"2026-09-21T14:13:20Z","provider":"claude-code","number":3,"email":"w@corp.com","reason":"invalid_grant"}"#,
            ),
            (
                AutoEvent::AccountUnquarantined {
                    provider: cc(),
                    number: 3,
                    email: "w@corp.com".into(),
                    reason: "account-replaced".into(),
                },
                r#"{"schemaVersion":1,"event":"account-unquarantined","ts":"2026-09-21T14:13:20Z","provider":"claude-code","number":3,"email":"w@corp.com","reason":"account-replaced"}"#,
            ),
            (
                AutoEvent::AllExhausted {
                    provider: cc(),
                    earliest_reset_at: Some(T0 + 9_630),
                },
                r#"{"schemaVersion":1,"event":"all-exhausted","ts":"2026-09-21T14:13:20Z","provider":"claude-code","earliestResetAt":"2026-09-21T16:53:50Z"}"#,
            ),
            (
                AutoEvent::AllExhausted {
                    provider: cc(),
                    earliest_reset_at: None,
                },
                r#"{"schemaVersion":1,"event":"all-exhausted","ts":"2026-09-21T14:13:20Z","provider":"claude-code","earliestResetAt":null}"#,
            ),
            (
                AutoEvent::Sleep {
                    provider: cc(),
                    seconds: 600.0,
                    until: T0 + 600,
                },
                r#"{"schemaVersion":1,"event":"sleep","ts":"2026-09-21T14:13:20Z","provider":"claude-code","seconds":600.0,"until":"2026-09-21T14:23:20Z"}"#,
            ),
            (
                AutoEvent::Sleep {
                    provider: cc(),
                    seconds: 66.04,
                    until: T0 + 66,
                },
                r#"{"schemaVersion":1,"event":"sleep","ts":"2026-09-21T14:13:20Z","provider":"claude-code","seconds":66.0,"until":"2026-09-21T14:14:26Z"}"#,
            ),
            (
                AutoEvent::Error {
                    provider: cc(),
                    message: "m".into(),
                    transient: true,
                },
                r#"{"schemaVersion":1,"event":"error","ts":"2026-09-21T14:13:20Z","provider":"claude-code","message":"m","transient":true}"#,
            ),
            (
                AutoEvent::ConfigWarning {
                    provider: cc(),
                    message: "m".into(),
                },
                r#"{"schemaVersion":1,"event":"config-warning","ts":"2026-09-21T14:13:20Z","provider":"claude-code","message":"m"}"#,
            ),
        ]
    }

    #[test]
    fn every_auto_kind_is_written_byte_for_byte_as_auto_wrote_it_before() {
        for (event, line) in auto_lines() {
            assert_eq!(event_json(&event, T0).to_string(), line);
        }
    }

    #[test]
    fn an_auto_kind_reaches_a_subscriber_exactly_as_auto_json_prints_it() {
        // D5.5: the socket and `auto --json` write the auto kinds from one function.
        for (event, line) in auto_lines() {
            assert_eq!(
                daemon_event_json(&DaemonEvent::Auto(event), T0).to_string(),
                line
            );
        }
    }

    #[test]
    fn each_daemon_kind_is_one_object_in_the_envelope_and_leaves_out_what_it_lacks() {
        let mode = |mode, waiting_for, holder_pid, backoff| DaemonEvent::Mode {
            provider: cc(),
            mode,
            waiting_for,
            holder_pid,
            backoff,
        };
        let cases = [
            (
                DaemonEvent::Usage {
                    provider: cc(),
                    updated: vec![1, 3],
                    fetch_errors: BTreeMap::new(),
                },
                r#"{"schemaVersion":1,"event":"usage","ts":"2026-09-21T14:13:20Z","provider":"claude-code","updated":[1,3]}"#,
            ),
            (
                DaemonEvent::Usage {
                    provider: cc(),
                    updated: Vec::new(),
                    fetch_errors: BTreeMap::from([(2, "http-429".into())]),
                },
                r#"{"schemaVersion":1,"event":"usage","ts":"2026-09-21T14:13:20Z","provider":"claude-code","updated":[],"fetchErrors":{"2":"http-429"}}"#,
            ),
            (
                DaemonEvent::Changed,
                r#"{"schemaVersion":1,"event":"changed","ts":"2026-09-21T14:13:20Z"}"#,
            ),
            (
                mode(Mode::Auto, None, None, None),
                r#"{"schemaVersion":1,"event":"mode","ts":"2026-09-21T14:13:20Z","provider":"claude-code","mode":"auto"}"#,
            ),
            (
                mode(Mode::Idle, None, None, None),
                r#"{"schemaVersion":1,"event":"mode","ts":"2026-09-21T14:13:20Z","provider":"claude-code","mode":"idle"}"#,
            ),
            (
                mode(
                    Mode::Waiting,
                    Some(WaitingFor::EngineLock),
                    Some(4242),
                    None,
                ),
                r#"{"schemaVersion":1,"event":"mode","ts":"2026-09-21T14:13:20Z","provider":"claude-code","mode":"waiting","waitingFor":"engine-lock","holderPid":4242}"#,
            ),
            (
                mode(Mode::Waiting, Some(WaitingFor::Schema), None, None),
                r#"{"schemaVersion":1,"event":"mode","ts":"2026-09-21T14:13:20Z","provider":"claude-code","mode":"waiting","waitingFor":"schema"}"#,
            ),
            (
                mode(Mode::Collect, None, None, Some(Backoff { until: T0 + 900 })),
                r#"{"schemaVersion":1,"event":"mode","ts":"2026-09-21T14:13:20Z","provider":"claude-code","mode":"collect","backoff":{"reason":"keychain","until":"2026-09-21T14:28:20Z"}}"#,
            ),
            (
                DaemonEvent::Stopping {
                    reason: StopReason::Signal,
                },
                r#"{"schemaVersion":1,"event":"stopping","ts":"2026-09-21T14:13:20Z","reason":"signal"}"#,
            ),
            (
                DaemonEvent::Stopping {
                    reason: StopReason::Upgrade,
                },
                r#"{"schemaVersion":1,"event":"stopping","ts":"2026-09-21T14:13:20Z","reason":"upgrade"}"#,
            ),
            (
                DaemonEvent::Stopping {
                    reason: StopReason::Schema,
                },
                r#"{"schemaVersion":1,"event":"stopping","ts":"2026-09-21T14:13:20Z","reason":"schema"}"#,
            ),
        ];
        for (event, line) in cases {
            assert_eq!(daemon_event_json(&event, T0).to_string(), line, "{event:?}");
        }
    }
}
