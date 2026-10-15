//! D5's wire format. Every event is one JSON object in §11.4's envelope, and `tagteam auto
//! --json`, `tagteam daemon --json` and the socket all write it from here (D3), so they never
//! differ. Plan B adds the requests, the replies and the greeting.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use tagteam_core::ProviderId;
use tagteam_core::time::format_iso8601;
use tagteam_engine::auto::AutoEvent;
use tagteam_engine::tokens::IndexState;

use crate::events::{Backoff, DaemonEvent};
use crate::status::{DaemonInfo, JobStatus, ProviderStatus, StatusSnapshot};

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
        DaemonEvent::TokenIndex { provider, state } => {
            ("token-index", Some(provider), index_state_json(state))
        }
    };
    envelope(kind, provider, fields, now_s)
}

/// The fields of `token-index` (D5.5's reserved kind; token report spec §12): the index's state
/// in camelCase, its times in ISO 8601 UTC, each null when the index has none.
fn index_state_json(s: &IndexState) -> Value {
    let iso = |ms: Option<i64>| ms.map(|ms| format_iso8601(ms.div_euclid(1000)));
    json!({
        "refreshedAt": iso(s.refreshed_at_ms),
        "changedAt": iso(s.changed_at_ms),
        "files": s.files,
        "responses": s.responses,
        "skippedLines": s.skipped_lines,
        "unreadableFiles": s.unreadable_files,
        "oldest": iso(s.oldest_ms),
        "newest": iso(s.newest_ms),
    })
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

// Protocol 1's requests, replies and status (D5.2–D5.4), and their typed mirror for the
// client (D5.7).

/// D5.2: the longest line either side sends, its newline left out.
pub const MAX_LINE: usize = 64 * 1024;
/// D5.3: the longest string `id` a request may carry, in bytes. A reply echoes the id, so this
/// bounds every reply line under `MAX_LINE`: an integer id is at most 20 bytes, a status reply
/// scales with the providers and jobs, and an event holds positions and short strings.
pub const MAX_ID_LEN: usize = 128;
/// D5.3's `error.type` for a request without a usable `id`, or without a string `op`.
pub const BAD_REQUEST: &str = "bad-request";
/// D5.3's `error.type` for an `op` this daemon does not know.
pub const UNKNOWN_OP: &str = "unknown-op";

/// D5.3: a request's `id`, which its reply carries back.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Int(i64),
    Str(String),
}

/// D5.3's ops. Protocol 1 is read-only (D1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Status,
    Subscribe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: RequestId,
    pub op: Op,
}

/// A request answered with `ok: false` (D5.3); the connection stays open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestError {
    /// The request's id when it had a usable one; the reply's `id` is null otherwise.
    pub id: Option<RequestId>,
    /// `BAD_REQUEST` or `UNKNOWN_OP`.
    pub kind: &'static str,
    pub message: String,
}

/// What a client's line comes to (Decision 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Request(Request),
    Refuse(RequestError),
    /// A framing violation (D5.2): the connection is closed, with no reply.
    Close,
}

/// Decision 1: one line from a client, its newline left out. A line over `MAX_LINE`, one that
/// is not UTF-8, and JSON that is not an object close the connection. An object without a
/// usable `id` (a string of at most `MAX_ID_LEN` bytes, or an integer that fits `i64`) or without a string `op` is a
/// `bad-request`, which carries the id when it was usable. An op this daemon does not know is
/// `unknown-op`. Unknown fields are ignored (D5.3). No message repeats the client's input.
pub fn parse_request(line: &[u8]) -> Parsed {
    if line.len() > MAX_LINE {
        return Parsed::Close;
    }
    let Ok(text) = std::str::from_utf8(line) else {
        return Parsed::Close;
    };
    let Ok(Value::Object(request)) = serde_json::from_str::<Value>(text) else {
        return Parsed::Close;
    };
    let refuse = |id: Option<RequestId>, kind: &'static str, message: &str| {
        Parsed::Refuse(RequestError {
            id,
            kind,
            message: message.to_owned(),
        })
    };
    let id = match request.get("id") {
        Some(Value::String(s)) if s.len() > MAX_ID_LEN => {
            return refuse(None, BAD_REQUEST, "the request id is longer than 128 bytes");
        }
        Some(Value::String(s)) => Some(RequestId::Str(s.clone())),
        Some(Value::Number(n)) => n.as_i64().map(RequestId::Int),
        _ => None,
    };
    let Some(id) = id else {
        return refuse(
            None,
            BAD_REQUEST,
            "a request needs an id that is a string or an integer",
        );
    };
    match request.get("op").and_then(Value::as_str) {
        Some("status") => Parsed::Request(Request { id, op: Op::Status }),
        Some("subscribe") => Parsed::Request(Request {
            id,
            op: Op::Subscribe,
        }),
        Some(_) => refuse(
            Some(id),
            UNKNOWN_OP,
            "this daemon knows the ops status and subscribe",
        ),
        None => refuse(
            Some(id),
            BAD_REQUEST,
            "a request needs an op that is a string",
        ),
    }
}

/// D5.2: the greeting, which the server writes first.
pub fn hello_json(d: &DaemonInfo) -> Value {
    json!({
        "schemaVersion": 1,
        "event": "hello",
        "protocol": PROTOCOL,
        "version": d.version,
        "pid": d.pid,
        "startedAt": format_iso8601(d.started_at),
    })
}

/// D5.3: a request's success.
pub fn ok_json(id: &RequestId, result: Value) -> Value {
    json!({"schemaVersion": 1, "id": id, "ok": true, "result": result})
}

/// D5.3: a request's failure; `id` is null when the request had no usable one.
pub fn error_json(e: &RequestError) -> Value {
    json!({
        "schemaVersion": 1,
        "id": e.id,
        "ok": false,
        "error": {"type": e.kind, "message": e.message},
    })
}

/// D5.4, §14.2: `path` as the log writes it, `~/…` under `home` and `~` for `home` itself. A
/// home of `/` rewrites nothing, since every path would be under it.
pub fn home_relative(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if home.parent().is_some() => {
            if rest.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                format!("~/{}", rest.display())
            }
        }
        _ => path.display().to_string(),
    }
}

/// D5.4's `status` result, built from the board's snapshot (Decision 12 of plan A), so it never
/// touches the store or waits for a job. Every key is written, null when empty.
pub fn status_json(s: &StatusSnapshot, home: &Path) -> Value {
    let d = &s.daemon;
    json!({
        "daemon": {
            "pid": d.pid,
            "version": d.version,
            "protocol": PROTOCOL,
            "startedAt": format_iso8601(d.started_at),
            "socket": d.socket.as_deref().map(|p| home_relative(p, home)),
            "log": home_relative(&d.log, home),
        },
        "notifications": {
            "enabled": s.notifications.enabled,
            "backend": s.notifications.backend,
            "events": s.notifications.events.iter().map(|e| e.as_str()).collect::<Vec<_>>(),
        },
        "providers": s.providers.iter().map(status_provider_json).collect::<Vec<_>>(),
        "jobs": s.jobs.iter().map(status_job_json).collect::<Vec<_>>(),
        "subscribers": s.subscribers,
    })
}

fn status_provider_json(p: &ProviderStatus) -> Value {
    json!({
        "provider": p.provider.as_str(),
        "mode": p.mode.as_str(),
        "autoswitch": p.autoswitch,
        "waitingFor": p.waiting_for.map(|w| w.as_str()),
        "holderPid": p.holder_pid,
        "backoff": p.backoff.map(status_backoff_json),
        "lastTickAt": p.last_tick_at.map(format_iso8601),
        "nextTickAt": p.next_tick_at.map(format_iso8601),
        "lastOutcome": p.last_outcome.as_ref().map(|o| json!({
            "kind": o.kind.as_str(),
            "reason": o.reason,
            "at": format_iso8601(o.at),
        })),
        "lastSwitch": p.last_switch.as_ref().map(|w| json!({
            "at": format_iso8601(w.at),
            "from": w.from,
            "to": w.to,
            "trigger": w.trigger,
        })),
    })
}

/// D5.4's `backoff`: the keychain backoff (D4.1) is the only one there is.
fn status_backoff_json(b: Backoff) -> Value {
    json!({"reason": "keychain", "until": format_iso8601(b.until)})
}

fn status_job_json(j: &JobStatus) -> Value {
    json!({
        "name": j.name,
        "lastRunAt": j.last_run_at.map(format_iso8601),
        "nextRunAt": j.next_run_at.map(format_iso8601),
        "lastError": j.last_error,
    })
}

/// D5.2's greeting, as a client reads it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol: u32,
    pub version: String,
    pub pid: u32,
    pub started_at: String,
}

/// D5.4's `status` result, as a client reads it. Times are ISO 8601 strings and paths are
/// written `~/…`, as the daemon wrote them. Fields a later daemon adds are ignored (D5.6).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub daemon: StatusDaemon,
    pub notifications: StatusNotifications,
    pub providers: Vec<StatusProvider>,
    pub jobs: Vec<StatusJob>,
    pub subscribers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusDaemon {
    pub pid: u32,
    pub version: String,
    pub protocol: u32,
    pub started_at: String,
    /// None when the daemon runs without its socket (D5.1).
    pub socket: Option<String>,
    pub log: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusNotifications {
    pub enabled: bool,
    /// `osascript`, `notify-send`, or none (D6).
    pub backend: Option<String>,
    pub events: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusProvider {
    pub provider: String,
    /// D4.3's mode: `auto`, `collect`, `waiting` or `idle`.
    pub mode: String,
    pub autoswitch: bool,
    /// `engine-lock` or `schema`.
    pub waiting_for: Option<String>,
    pub holder_pid: Option<u32>,
    pub backoff: Option<StatusBackoff>,
    pub last_tick_at: Option<String>,
    pub next_tick_at: Option<String>,
    pub last_outcome: Option<StatusOutcome>,
    pub last_switch: Option<StatusSwitch>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusBackoff {
    /// `keychain` (D4.1).
    pub reason: String,
    pub until: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusOutcome {
    /// `switch`, `no-switch`, `collect` or `error`.
    pub kind: String,
    pub reason: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusSwitch {
    pub at: String,
    pub from: u32,
    pub to: u32,
    pub trigger: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusJob {
    pub name: String,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub last_error: Option<String>,
}

/// An event as a subscriber reads it (D5.5, D5.7).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// One of §11.4's kinds; `raw` is the whole object, as `auto --json` prints it.
    Auto {
        kind: String,
        provider: String,
        raw: Value,
    },
    /// Positions whose reading a `collect` round recorded, and each failed fetch's error kind.
    Usage {
        provider: String,
        updated: Vec<u32>,
        fetch_errors: BTreeMap<u32, String>,
    },
    /// Another process committed to the store (D4.4).
    Changed,
    Mode {
        provider: String,
        mode: String,
        waiting_for: Option<String>,
        holder_pid: Option<u32>,
        backoff: Option<StatusBackoff>,
    },
    /// The last event before the daemon closes: `signal`, `upgrade` or `schema`.
    Stopping { reason: String },
    /// D5.5's `token-index`; `raw` is the whole object, whose fields token report §12 defines.
    TokenIndex { provider: String, raw: Value },
    /// A kind this client does not know, or a known kind whose fields it cannot read (D5.6).
    Unknown { kind: String, raw: Value },
}

/// §11.4's kinds: what `auto --json` prints, carried unchanged on the socket (D5.5).
const AUTO_EVENT_KINDS: [&str; 9] = [
    "poll",
    "switch",
    "no-switch",
    "account-quarantined",
    "account-unquarantined",
    "all-exhausted",
    "sleep",
    "error",
    "config-warning",
];

/// D5.7: a line's event, typed. `None` for anything that is not an object with a string
/// `event`; a reply is one such thing.
pub fn parse_event(v: &Value) -> Option<Event> {
    let o = v.as_object()?;
    let kind = o.get("event")?.as_str()?;
    Some(typed_event(kind, o, v).unwrap_or_else(|| Event::Unknown {
        kind: kind.to_owned(),
        raw: v.clone(),
    }))
}

/// `kind`'s typed event, or `None` when the kind is not one this client knows or its fields
/// are not what D5.5 gives it.
fn typed_event(kind: &str, o: &Map<String, Value>, raw: &Value) -> Option<Event> {
    let provider = || o.get("provider")?.as_str().map(str::to_owned);
    Some(match kind {
        _ if AUTO_EVENT_KINDS.contains(&kind) => Event::Auto {
            kind: kind.to_owned(),
            provider: provider()?,
            raw: raw.clone(),
        },
        "usage" => Event::Usage {
            provider: provider()?,
            updated: read_positions(o.get("updated"))?,
            fetch_errors: read_by_position(o.get("fetchErrors"))?,
        },
        "changed" => Event::Changed,
        "mode" => Event::Mode {
            provider: provider()?,
            mode: o.get("mode")?.as_str()?.to_owned(),
            waiting_for: nullable_str(o.get("waitingFor"))?,
            holder_pid: nullable_u32(o.get("holderPid"))?,
            backoff: match o.get("backoff") {
                None | Some(Value::Null) => None,
                Some(b) => Some(serde_json::from_value(b.clone()).ok()?),
            },
        },
        "stopping" => Event::Stopping {
            reason: o.get("reason")?.as_str()?.to_owned(),
        },
        "token-index" => Event::TokenIndex {
            provider: provider()?,
            raw: raw.clone(),
        },
        _ => return None,
    })
}

/// An optional string field: `Some(None)` when absent or null, `None` when it holds anything
/// but a string.
fn nullable_str(v: Option<&Value>) -> Option<Option<String>> {
    match v {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.clone())),
        Some(_) => None,
    }
}

/// An optional `u32` field, read as `nullable_str` reads a string.
fn nullable_u32(v: Option<&Value>) -> Option<Option<u32>> {
    match v {
        None | Some(Value::Null) => Some(None),
        Some(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()).map(Some),
    }
}

/// `usage`'s `updated`: account positions, none when absent or null.
fn read_positions(v: Option<&Value>) -> Option<Vec<u32>> {
    match v {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|n| n.as_u64().and_then(|n| u32::try_from(n).ok()))
            .collect(),
        Some(_) => None,
    }
}

/// `usage`'s `fetchErrors`: error kinds keyed by position, as §11.4's objects key accounts.
fn read_by_position(v: Option<&Value>) -> Option<BTreeMap<u32, String>> {
    match v {
        None | Some(Value::Null) => Some(BTreeMap::new()),
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| Some((k.parse().ok()?, v.as_str()?.to_owned())))
            .collect(),
        Some(_) => None,
    }
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

#[cfg(test)]
mod socket_protocol_tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};
    use tagteam_core::ProviderId;
    use tagteam_core::autoswitch::Trigger;
    use tagteam_core::time::format_iso8601;
    use tagteam_engine::auto::AutoEvent;
    use tagteam_engine::settings::NotifyEvent;

    use super::*;
    use crate::events::{Backoff, DaemonEvent, Mode, StopReason, WaitingFor};
    use crate::status::{
        DaemonInfo, JobStatus, LastOutcome, LastSwitch, NotifyInfo, OutcomeKind, ProviderStatus,
        StatusSnapshot,
    };

    /// 2026-09-21T14:13:20Z.
    const T: i64 = 1_790_000_000;
    const HOME: &str = "/Users/tester";

    fn iso(s: i64) -> String {
        format_iso8601(s)
    }

    /// `parse_request`'s answer without the message: the request, a refusal's id and kind, or
    /// `(None, "close")`.
    type Class = Result<Request, (Option<RequestId>, &'static str)>;

    fn class(line: &[u8]) -> Class {
        match parse_request(line) {
            Parsed::Request(r) => Ok(r),
            Parsed::Refuse(e) => {
                assert!(!e.message.is_empty(), "a refusal says why");
                Err((e.id, e.kind))
            }
            Parsed::Close => Err((None, "close")),
        }
    }

    fn check(line: &[u8], want: Class) {
        assert_eq!(class(line), want, "{}", String::from_utf8_lossy(line));
    }

    #[test]
    fn requests_are_classified_as_decision_1_says() {
        use RequestId::{Int, Str};
        let status = |id| Ok(Request { id, op: Op::Status });
        check(br#"{"id":1,"op":"status"}"#, status(Int(1)));
        check(
            br#"{"id":"tui-7","op":"subscribe"}"#,
            Ok(Request {
                id: Str("tui-7".into()),
                op: Op::Subscribe,
            }),
        );
        // D5.3: unknown fields are ignored, whatever their order.
        check(
            br#"{"op":"status","id":-3,"since":"later"}"#,
            status(Int(-3)),
        );
        check(br#"  {"id":2,"op":"status"}  "#, status(Int(2)));
        // No usable id: bad-request, with a null id.
        let no_usable_id: [&[u8]; 9] = [
            br#"{"op":"status"}"#,
            br#"{"id":1.5,"op":"status"}"#,
            br#"{"id":2.0,"op":"status"}"#,
            br#"{"id":1e2,"op":"status"}"#,
            br#"{"id":null,"op":"status"}"#,
            br#"{"id":true,"op":"status"}"#,
            br#"{"id":[1],"op":"status"}"#,
            br#"{"id":9223372036854775808,"op":"status"}"#,
            br#"{"op":"pause"}"#,
        ];
        for line in no_usable_id {
            check(line, Err((None, BAD_REQUEST)));
        }
        // A usable id and no string op: bad-request, with the id.
        check(br#"{"id":7}"#, Err((Some(Int(7)), BAD_REQUEST)));
        check(br#"{"id":7,"op":3}"#, Err((Some(Int(7)), BAD_REQUEST)));
        // An op this daemon does not know: unknown-op, with the id.
        check(
            br#"{"id":"x","op":"pause"}"#,
            Err((Some(Str("x".into())), UNKNOWN_OP)),
        );
        check(
            br#"{"id":8,"op":"Status"}"#,
            Err((Some(Int(8)), UNKNOWN_OP)),
        );
        // Framing violations close the connection (D5.2).
        let closing: [&[u8]; 9] = [
            b"[1,2]",
            b"\"status\"",
            b"42",
            b"null",
            b"",
            b"not json",
            br#"{"id":1,"op":"status"} {"id":2}"#,
            b"{\"id\":\"\xff\",\"op\":\"status\"}",
            b"\xfe\xff",
        ];
        for line in closing {
            check(line, Err((None, "close")));
        }
        // D5.3: a string id of 128 bytes is echoed, one of 129 is refused with a null id.
        let with_id = |len: usize| format!(r#"{{"id":"{}","op":"status"}}"#, "i".repeat(len));
        check(
            with_id(MAX_ID_LEN).as_bytes(),
            Ok(Request {
                id: Str("i".repeat(MAX_ID_LEN)),
                op: Op::Status,
            }),
        );
        check(with_id(MAX_ID_LEN + 1).as_bytes(), Err((None, BAD_REQUEST)));
        // Bytes, not characters: 65 two-byte characters are 130 bytes.
        check(
            format!(r#"{{"id":"{}","op":"status"}}"#, "é".repeat(65)).as_bytes(),
            Err((None, BAD_REQUEST)),
        );
        // The length limit leaves out the newline: a request padded to it is read, one byte
        // more closes.
        let padded = |len: usize| {
            let mut line = br#"{"id":1,"op":"status"}"#.to_vec();
            line.resize(len, b' ');
            line
        };
        check(padded(MAX_LINE).as_slice(), status(Int(1)));
        check(padded(MAX_LINE + 1).as_slice(), Err((None, "close")));
    }

    #[test]
    fn replies_have_d5_3s_shapes() {
        assert_eq!(
            ok_json(&RequestId::Int(7), json!({})).to_string(),
            r#"{"schemaVersion":1,"id":7,"ok":true,"result":{}}"#
        );
        assert_eq!(
            ok_json(&RequestId::Str("a".into()), json!({"x": 1})).to_string(),
            r#"{"schemaVersion":1,"id":"a","ok":true,"result":{"x":1}}"#
        );
        let refused = RequestError {
            id: Some(RequestId::Int(3)),
            kind: UNKNOWN_OP,
            message: "m".into(),
        };
        assert_eq!(
            error_json(&refused).to_string(),
            r#"{"schemaVersion":1,"id":3,"ok":false,"error":{"type":"unknown-op","message":"m"}}"#
        );
        let anonymous = RequestError {
            id: None,
            kind: BAD_REQUEST,
            message: "m".into(),
        };
        assert_eq!(
            error_json(&anonymous).to_string(),
            r#"{"schemaVersion":1,"id":null,"ok":false,"error":{"type":"bad-request","message":"m"}}"#
        );
    }

    #[test]
    fn request_ids_round_trip_through_a_value() {
        // The client reads a reply's id through a `Value` (Task 3), and so does this.
        assert_eq!(serde_json::to_value(RequestId::Int(7)).unwrap(), json!(7));
        assert_eq!(
            serde_json::to_value(RequestId::Str("a".into())).unwrap(),
            json!("a")
        );
        assert_eq!(
            serde_json::from_value::<RequestId>(json!(7)).unwrap(),
            RequestId::Int(7)
        );
        assert_eq!(
            serde_json::from_value::<RequestId>(json!(-7)).unwrap(),
            RequestId::Int(-7)
        );
        assert_eq!(
            serde_json::from_value::<RequestId>(json!("a")).unwrap(),
            RequestId::Str("a".into())
        );
        assert!(serde_json::from_value::<RequestId>(json!(1.5)).is_err());
    }

    fn daemon_info() -> DaemonInfo {
        let home = Path::new(HOME);
        DaemonInfo {
            pid: 4242,
            version: "20261014.2045.0".into(),
            started_at: T,
            socket: Some(home.join(".local/share/tagteam/daemon.sock")),
            log: home.join(".local/state/tagteam/tagteam.log"),
        }
    }

    #[test]
    fn the_greeting_has_d5_2s_shape_and_reads_back_as_hello() {
        let v = hello_json(&daemon_info());
        assert_eq!(
            v.to_string(),
            r#"{"schemaVersion":1,"event":"hello","protocol":1,"version":"20261014.2045.0","pid":4242,"startedAt":"2026-09-21T14:13:20Z"}"#
        );
        assert_eq!(
            serde_json::from_value::<Hello>(v).unwrap(),
            Hello {
                protocol: 1,
                version: "20261014.2045.0".into(),
                pid: 4242,
                started_at: "2026-09-21T14:13:20Z".into(),
            }
        );
    }

    #[test]
    fn paths_under_home_are_written_from_a_tilde() {
        // D5.4: "as in the log" (§14.2).
        let home = Path::new(HOME);
        let rel = |p: &str| home_relative(Path::new(p), home);
        assert_eq!(
            rel("/Users/tester/.local/share/tagteam/daemon.sock"),
            "~/.local/share/tagteam/daemon.sock"
        );
        assert_eq!(rel("/Users/tester"), "~");
        assert_eq!(
            rel("/Users/tester2/x"),
            "/Users/tester2/x",
            "a sibling that shares the prefix is not under home"
        );
        assert_eq!(
            rel("/srv/data/tagteam/daemon.sock"),
            "/srv/data/tagteam/daemon.sock"
        );
        assert_eq!(
            home_relative(Path::new("/Users/tester/x"), Path::new("/Users/tester/")),
            "~/x"
        );
        assert_eq!(
            home_relative(Path::new("/x/y"), Path::new("/")),
            "/x/y",
            "a home of / would rewrite every path, so it rewrites none"
        );
    }

    fn provider(id: &str, mode: Mode) -> ProviderStatus {
        ProviderStatus {
            provider: ProviderId::new(id),
            mode,
            autoswitch: true,
            waiting_for: None,
            holder_pid: None,
            backoff: None,
            last_tick_at: None,
            next_tick_at: None,
            last_outcome: None,
            last_switch: None,
        }
    }

    /// A board with a provider in each of D4.3's modes, one in a keychain backoff.
    fn snapshot() -> StatusSnapshot {
        StatusSnapshot {
            daemon: daemon_info(),
            notifications: NotifyInfo {
                enabled: true,
                backend: Some("osascript"),
                events: vec![
                    NotifyEvent::Switch,
                    NotifyEvent::Quarantine,
                    NotifyEvent::Limit,
                ],
            },
            providers: vec![
                ProviderStatus {
                    last_tick_at: Some(T + 60),
                    next_tick_at: Some(T + 360),
                    last_outcome: Some(LastOutcome {
                        kind: OutcomeKind::NoSwitch,
                        reason: Some("below-threshold".into()),
                        at: T + 60,
                    }),
                    last_switch: Some(LastSwitch {
                        at: T + 30,
                        from: 1,
                        to: 2,
                        trigger: "proactive".into(),
                    }),
                    ..provider("claude-code", Mode::Auto)
                },
                ProviderStatus {
                    autoswitch: false,
                    backoff: Some(Backoff { until: T + 120 }),
                    last_tick_at: Some(T + 60),
                    next_tick_at: Some(T + 120),
                    last_outcome: Some(LastOutcome {
                        kind: OutcomeKind::Collect,
                        reason: None,
                        at: T + 60,
                    }),
                    ..provider("fake", Mode::Collect)
                },
                ProviderStatus {
                    waiting_for: Some(WaitingFor::EngineLock),
                    holder_pid: Some(5151),
                    next_tick_at: Some(T + 60),
                    ..provider("third", Mode::Waiting)
                },
                ProviderStatus {
                    waiting_for: Some(WaitingFor::Schema),
                    ..provider("fourth", Mode::Waiting)
                },
                ProviderStatus {
                    last_tick_at: Some(T),
                    next_tick_at: Some(T + 60),
                    last_outcome: Some(LastOutcome {
                        kind: OutcomeKind::Error,
                        reason: Some("the store is locked".into()),
                        at: T,
                    }),
                    ..provider("fifth", Mode::Idle)
                },
            ],
            jobs: vec![
                JobStatus {
                    name: "store-watch".into(),
                    last_run_at: Some(T + 58),
                    next_run_at: Some(T + 60),
                    last_error: None,
                },
                JobStatus {
                    name: "upgrade-watch".into(),
                    last_run_at: Some(T),
                    next_run_at: Some(T + 60),
                    last_error: Some("the program path resolves to nothing".into()),
                },
            ],
            subscribers: 2,
        }
    }

    #[test]
    fn status_json_matches_d5_4_for_a_provider_in_each_mode() {
        let want = json!({
            "daemon": {
                "pid": 4242, "version": "20261014.2045.0", "protocol": 1,
                "startedAt": "2026-09-21T14:13:20Z",
                "socket": "~/.local/share/tagteam/daemon.sock",
                "log": "~/.local/state/tagteam/tagteam.log",
            },
            "notifications": {
                "enabled": true, "backend": "osascript",
                "events": ["switch", "quarantine", "limit"],
            },
            "providers": [
                {
                    "provider": "claude-code", "mode": "auto", "autoswitch": true,
                    "waitingFor": null, "holderPid": null, "backoff": null,
                    "lastTickAt": "2026-09-21T14:14:20Z", "nextTickAt": "2026-09-21T14:19:20Z",
                    "lastOutcome": {"kind": "no-switch", "reason": "below-threshold", "at": "2026-09-21T14:14:20Z"},
                    "lastSwitch": {"at": "2026-09-21T14:13:50Z", "from": 1, "to": 2, "trigger": "proactive"},
                },
                {
                    "provider": "fake", "mode": "collect", "autoswitch": false,
                    "waitingFor": null, "holderPid": null,
                    "backoff": {"reason": "keychain", "until": "2026-09-21T14:15:20Z"},
                    "lastTickAt": "2026-09-21T14:14:20Z", "nextTickAt": "2026-09-21T14:15:20Z",
                    "lastOutcome": {"kind": "collect", "reason": null, "at": "2026-09-21T14:14:20Z"},
                    "lastSwitch": null,
                },
                {
                    "provider": "third", "mode": "waiting", "autoswitch": true,
                    "waitingFor": "engine-lock", "holderPid": 5151, "backoff": null,
                    "lastTickAt": null, "nextTickAt": "2026-09-21T14:14:20Z",
                    "lastOutcome": null, "lastSwitch": null,
                },
                {
                    "provider": "fourth", "mode": "waiting", "autoswitch": true,
                    "waitingFor": "schema", "holderPid": null, "backoff": null,
                    "lastTickAt": null, "nextTickAt": null,
                    "lastOutcome": null, "lastSwitch": null,
                },
                {
                    "provider": "fifth", "mode": "idle", "autoswitch": true,
                    "waitingFor": null, "holderPid": null, "backoff": null,
                    "lastTickAt": "2026-09-21T14:13:20Z", "nextTickAt": "2026-09-21T14:14:20Z",
                    "lastOutcome": {"kind": "error", "reason": "the store is locked", "at": "2026-09-21T14:13:20Z"},
                    "lastSwitch": null,
                },
            ],
            "jobs": [
                {"name": "store-watch", "lastRunAt": "2026-09-21T14:14:18Z",
                 "nextRunAt": "2026-09-21T14:14:20Z", "lastError": null},
                {"name": "upgrade-watch", "lastRunAt": "2026-09-21T14:13:20Z",
                 "nextRunAt": "2026-09-21T14:14:20Z", "lastError": "the program path resolves to nothing"},
            ],
            "subscribers": 2,
        });
        // Compared as text, so the key order is pinned too (`preserve_order`).
        assert_eq!(
            serde_json::to_string_pretty(&status_json(&snapshot(), Path::new(HOME))).unwrap(),
            serde_json::to_string_pretty(&want).unwrap()
        );
        // No socket (D5.1's bind failure), and a data directory outside home.
        let mut s = snapshot();
        s.daemon.socket = None;
        assert_eq!(
            status_json(&s, Path::new(HOME))["daemon"]["socket"],
            Value::Null
        );
        s.daemon.socket = Some(PathBuf::from("/srv/data/tagteam/daemon.sock"));
        assert_eq!(
            status_json(&s, Path::new(HOME))["daemon"]["socket"],
            "/srv/data/tagteam/daemon.sock"
        );
    }

    #[test]
    fn status_json_reads_back_as_the_typed_status() {
        let v = status_json(&snapshot(), Path::new(HOME));
        let status: Status = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(
            status.daemon,
            StatusDaemon {
                pid: 4242,
                version: "20261014.2045.0".into(),
                protocol: 1,
                started_at: iso(T),
                socket: Some("~/.local/share/tagteam/daemon.sock".into()),
                log: "~/.local/state/tagteam/tagteam.log".into(),
            }
        );
        assert_eq!(
            status.notifications,
            StatusNotifications {
                enabled: true,
                backend: Some("osascript".into()),
                events: vec!["switch".into(), "quarantine".into(), "limit".into()],
            }
        );
        assert_eq!(
            status.providers[0],
            StatusProvider {
                provider: "claude-code".into(),
                mode: "auto".into(),
                autoswitch: true,
                waiting_for: None,
                holder_pid: None,
                backoff: None,
                last_tick_at: Some(iso(T + 60)),
                next_tick_at: Some(iso(T + 360)),
                last_outcome: Some(StatusOutcome {
                    kind: "no-switch".into(),
                    reason: Some("below-threshold".into()),
                    at: iso(T + 60),
                }),
                last_switch: Some(StatusSwitch {
                    at: iso(T + 30),
                    from: 1,
                    to: 2,
                    trigger: "proactive".into(),
                }),
            }
        );
        assert_eq!(
            status.providers[1].backoff,
            Some(StatusBackoff {
                reason: "keychain".into(),
                until: iso(T + 120),
            })
        );
        assert_eq!(
            status.providers[2].waiting_for.as_deref(),
            Some("engine-lock")
        );
        assert_eq!(status.providers[2].holder_pid, Some(5151));
        assert_eq!(status.providers[3].waiting_for.as_deref(), Some("schema"));
        assert_eq!(status.providers[4].mode, "idle");
        assert_eq!(
            status.jobs[1],
            StatusJob {
                name: "upgrade-watch".into(),
                last_run_at: Some(iso(T)),
                next_run_at: Some(iso(T + 60)),
                last_error: Some("the program path resolves to nothing".into()),
            }
        );
        assert_eq!(status.subscribers, 2);
        // D5.6: a field a later daemon adds is ignored.
        let mut later = v;
        later["daemon"]["uptime"] = json!(5);
        later["providers"][0]["queue"] = json!([]);
        assert_eq!(serde_json::from_value::<Status>(later).unwrap(), status);
    }

    #[test]
    fn every_daemon_kind_reads_back_from_daemon_event_json() {
        // D5.5's daemon kinds, through the function the hub writes them with (Task 2).
        let cc = || ProviderId::new("claude-code");
        let errors = BTreeMap::from([(2, "rate-limited".to_owned())]);
        let cases = vec![
            (
                DaemonEvent::Usage {
                    provider: cc(),
                    updated: vec![1, 3],
                    fetch_errors: errors.clone(),
                },
                Event::Usage {
                    provider: "claude-code".into(),
                    updated: vec![1, 3],
                    fetch_errors: errors,
                },
            ),
            (
                DaemonEvent::Usage {
                    provider: cc(),
                    updated: vec![],
                    fetch_errors: BTreeMap::new(),
                },
                Event::Usage {
                    provider: "claude-code".into(),
                    updated: vec![],
                    fetch_errors: BTreeMap::new(),
                },
            ),
            (DaemonEvent::Changed, Event::Changed),
            (
                DaemonEvent::Mode {
                    provider: cc(),
                    mode: Mode::Waiting,
                    waiting_for: Some(WaitingFor::EngineLock),
                    holder_pid: Some(4242),
                    backoff: None,
                },
                Event::Mode {
                    provider: "claude-code".into(),
                    mode: "waiting".into(),
                    waiting_for: Some("engine-lock".into()),
                    holder_pid: Some(4242),
                    backoff: None,
                },
            ),
            (
                DaemonEvent::Mode {
                    provider: cc(),
                    mode: Mode::Collect,
                    waiting_for: None,
                    holder_pid: None,
                    backoff: Some(Backoff { until: T + 60 }),
                },
                Event::Mode {
                    provider: "claude-code".into(),
                    mode: "collect".into(),
                    waiting_for: None,
                    holder_pid: None,
                    backoff: Some(StatusBackoff {
                        reason: "keychain".into(),
                        until: iso(T + 60),
                    }),
                },
            ),
            (
                DaemonEvent::Stopping {
                    reason: StopReason::Upgrade,
                },
                Event::Stopping {
                    reason: "upgrade".into(),
                },
            ),
            (
                DaemonEvent::Stopping {
                    reason: StopReason::Schema,
                },
                Event::Stopping {
                    reason: "schema".into(),
                },
            ),
        ];
        for (sent, want) in cases {
            let v = daemon_event_json(&sent, T);
            assert_eq!(parse_event(&v), Some(want), "{v}");
        }
    }

    #[test]
    fn auto_kinds_read_back_as_auto_with_their_whole_object() {
        // D5.5: the auto kinds are `auto --json`'s objects, and the client keeps each whole.
        let cc = ProviderId::new("claude-code");
        let switch = AutoEvent::Switch {
            provider: cc.clone(),
            trigger: Trigger::Proactive,
            from: 1,
            to: 2,
            warnings: vec![],
            dry_run: false,
        };
        let quarantined = AutoEvent::AccountQuarantined {
            provider: cc,
            number: 2,
            email: "b@example.com".into(),
            reason: "refresh-revoked".into(),
        };
        for (e, kind) in [(switch, "switch"), (quarantined, "account-quarantined")] {
            let v = event_json(&e, T);
            let want = Event::Auto {
                kind: kind.into(),
                provider: "claude-code".into(),
                raw: v.clone(),
            };
            assert_eq!(parse_event(&v), Some(want.clone()));
            assert_eq!(
                parse_event(&daemon_event_json(&DaemonEvent::Auto(e), T)),
                Some(want),
                "the socket writes them as auto --json does"
            );
        }
    }

    #[test]
    fn unknown_and_unreadable_events() {
        // D5.6: a kind this client does not know, and a known kind whose fields it cannot
        // read, are `Unknown`, whole; anything that is not an event object is no event.
        let future = json!({"schemaVersion":1,"event":"some-future-kind","ts":"2026-09-21T14:13:20Z","files":3});
        assert_eq!(
            parse_event(&future),
            Some(Event::Unknown {
                kind: "some-future-kind".into(),
                raw: future.clone(),
            })
        );
        for raw in [
            json!({"schemaVersion":1,"event":"mode","ts":"x","mode":"auto"}),
            json!({"schemaVersion":1,"event":"mode","ts":"x","provider":"p","mode":"auto","holderPid":-1}),
            json!({"schemaVersion":1,"event":"usage","ts":"x","provider":"p","updated":[-1]}),
            json!({"schemaVersion":1,"event":"usage","ts":"x","provider":"p","fetchErrors":{"two":"x"}}),
            json!({"schemaVersion":1,"event":"stopping","ts":"x"}),
            json!({"schemaVersion":1,"event":"switch","ts":"x"}),
        ] {
            let kind = raw["event"].as_str().unwrap().to_owned();
            assert_eq!(
                parse_event(&raw),
                Some(Event::Unknown {
                    kind,
                    raw: raw.clone(),
                }),
                "{raw}"
            );
        }
        // D5.5's optional fields read as none, absent or null.
        let bare = json!({"schemaVersion":1,"event":"mode","ts":"x","provider":"p","mode":"auto"});
        assert_eq!(
            parse_event(&bare),
            Some(Event::Mode {
                provider: "p".into(),
                mode: "auto".into(),
                waiting_for: None,
                holder_pid: None,
                backoff: None,
            })
        );
        let nulls = json!({"schemaVersion":1,"event":"mode","ts":"x","provider":"p","mode":"auto",
            "waitingFor":null,"holderPid":null,"backoff":null});
        assert_eq!(parse_event(&nulls), parse_event(&bare));
        let usage = json!({"schemaVersion":1,"event":"usage","ts":"x","provider":"p"});
        assert_eq!(
            parse_event(&usage),
            Some(Event::Usage {
                provider: "p".into(),
                updated: vec![],
                fetch_errors: BTreeMap::new(),
            })
        );
        for not_an_event in [
            json!([1]),
            json!("changed"),
            json!({"schemaVersion":1,"id":1,"ok":true,"result":{}}),
            json!({"event":7}),
        ] {
            assert_eq!(parse_event(&not_an_event), None, "{not_an_event}");
        }
    }
}
