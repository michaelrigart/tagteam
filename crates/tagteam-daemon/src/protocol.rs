//! D5's wire format. Every event is one JSON object in §11.4's envelope, and `tagteam auto
//! --json`, `tagteam daemon --json` and the socket all write it from here (D3), so they never
//! differ. It holds the requests, the replies and the greeting too.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use tagteam_core::ProviderId;
use tagteam_core::autoswitch::{NoSwitchReason, Trigger};
use tagteam_core::time::format_iso8601;
use tagteam_engine::auto::AutoEvent;
use tagteam_engine::settings::NotifyEvent;
use tagteam_engine::tokens::IndexState;

use crate::events::{Backoff, DaemonEvent, Mode, StopReason, WaitingFor};
use crate::status::{DaemonInfo, JobStatus, OutcomeKind, ProviderStatus, StatusSnapshot};

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
            counting_window: _,
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
    envelope(kind, Some(provider), fields, now_s)
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

/// A closed set of wire spellings: each value has one, and `parse` reads it back exactly.
pub trait Spelled: Sized + Copy {
    fn parse(s: &str) -> Option<Self>;
    fn as_str(self) -> &'static str;
}

macro_rules! spelled {
    ($($t:ty),*) => {$(
        impl Spelled for $t {
            fn parse(s: &str) -> Option<Self> {
                <$t>::parse(s)
            }

            fn as_str(self) -> &'static str {
                <$t>::as_str(self)
            }
        }
    )*};
}

spelled!(
    Mode,
    WaitingFor,
    StopReason,
    OutcomeKind,
    NoSwitchReason,
    Trigger,
    NotifyEvent
);

/// A value as the wire spells it: one this client knows, or the spelling of one a later daemon
/// added (D5.6), kept as it came.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wire<T> {
    Known(T),
    Other(String),
}

impl<T: Spelled> Wire<T> {
    pub fn from_wire(s: &str) -> Self {
        T::parse(s).map_or_else(|| Wire::Other(s.to_owned()), Wire::Known)
    }

    /// The spelling, whether or not this client knows it.
    pub fn as_str(&self) -> &str {
        match self {
            Wire::Known(v) => v.as_str(),
            Wire::Other(s) => s,
        }
    }

    pub fn known(&self) -> Option<T> {
        match self {
            Wire::Known(v) => Some(*v),
            Wire::Other(_) => None,
        }
    }
}

/// A spelling read from a string, kept as it came when this client does not know it (D5.6).
impl<'de, T: Spelled> serde::Deserialize<'de> for Wire<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Wire::from_wire(&s))
    }
}

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

/// D5.4's `status` result, built from the board's snapshot (daemon plan A, Decision 12), so it
/// never touches the store or waits for a job. Every key is written, null when empty.
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
    pub events: Vec<Wire<NotifyEvent>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusProvider {
    pub provider: String,
    /// D4.3's mode: `auto`, `collect`, `waiting` or `idle`.
    pub mode: Wire<Mode>,
    pub autoswitch: bool,
    /// `engine-lock` or `schema`.
    pub waiting_for: Option<Wire<WaitingFor>>,
    pub holder_pid: Option<u32>,
    pub backoff: Option<StatusBackoff>,
    pub last_tick_at: Option<String>,
    pub next_tick_at: Option<String>,
    pub last_outcome: Option<StatusOutcome>,
    pub last_switch: Option<StatusSwitch>,
}

impl StatusProvider {
    /// The daemon job collects usage for this provider itself: `auto`, `collect`, or `waiting`
    /// for `engine-lock`. False for anything else, an unknown value included.
    pub fn collects_for_itself(&self) -> bool {
        match self.mode.known() {
            Some(Mode::Auto | Mode::Collect) => true,
            Some(Mode::Waiting) => self.waits_for() == Some(WaitingFor::EngineLock),
            Some(Mode::Idle) | None => false,
        }
    }

    /// The job waits for a newer binary (`waiting` for `schema`). False for anything else.
    pub fn waits_for_upgrade(&self) -> bool {
        self.mode.known() == Some(Mode::Waiting) && self.waits_for() == Some(WaitingFor::Schema)
    }

    fn waits_for(&self) -> Option<WaitingFor> {
        self.waiting_for.as_ref().and_then(Wire::known)
    }
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
    pub kind: Wire<OutcomeKind>,
    pub reason: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusSwitch {
    pub at: String,
    pub from: u32,
    pub to: u32,
    pub trigger: Wire<Trigger>,
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
        mode: Wire<Mode>,
        waiting_for: Option<Wire<WaitingFor>>,
        holder_pid: Option<u32>,
        backoff: Option<StatusBackoff>,
    },
    /// The last event before the daemon closes: `signal`, `upgrade` or `schema`.
    Stopping { reason: Wire<StopReason> },
    /// D5.5's `token-index`; `raw` is the whole object, whose fields token report §12 defines.
    TokenIndex { provider: String, raw: Value },
    /// A kind this client does not know, or a known kind whose fields it cannot read (D5.6).
    Unknown { kind: String, raw: Value },
}

impl Event {
    /// `Some` only for `Event::Mode`.
    pub fn mode(&self) -> Option<Wire<Mode>> {
        match self {
            Event::Mode { mode, .. } => Some(mode.clone()),
            _ => None,
        }
    }

    /// `Some` only for an `Event::Mode` that names what it waits for.
    pub fn waiting_for(&self) -> Option<Wire<WaitingFor>> {
        match self {
            Event::Mode { waiting_for, .. } => waiting_for.clone(),
            _ => None,
        }
    }

    /// `Some` only for an `Event::Auto` of kind `poll` whose object parses.
    pub fn poll(&self) -> Option<PollPayload> {
        self.auto_raw("poll").and_then(parse_poll)
    }

    /// `Some` only for an `Event::Auto` of kind `switch` that names `from` and `to`.
    pub fn switch(&self) -> Option<SwitchPayload> {
        self.auto_raw("switch").and_then(parse_switch)
    }

    /// `Some` only for an `Event::Auto` of kind `no-switch`.
    pub fn no_switch(&self) -> Option<NoSwitchPayload> {
        self.auto_raw("no-switch").and_then(parse_no_switch)
    }

    /// `poll`, `switch`, `account-quarantined` or `account-unquarantined`: an account's
    /// reading or state changed (D5.5).
    pub fn changes_an_account(&self) -> bool {
        matches!(self, Event::Auto { kind, .. } if ACCOUNT_KINDS.contains(&kind.as_str()))
    }

    /// `Some` only for an `Event::Auto` of kind `all-exhausted`; a missing or null
    /// `earliestResetAt` is `None`.
    pub fn all_exhausted(&self) -> Option<AllExhaustedPayload> {
        let raw = self.auto_raw("all-exhausted")?;
        Some(AllExhaustedPayload {
            earliest_reset_at: optional_text(raw, "earliestResetAt"),
        })
    }

    /// An `error`'s `message`; a missing one reads as empty.
    pub fn error(&self) -> Option<String> {
        self.auto_raw("error").map(|raw| text(raw, "message"))
    }

    /// A `config-warning`'s `message`; a missing one reads as empty.
    pub fn config_warning(&self) -> Option<String> {
        self.auto_raw("config-warning")
            .map(|raw| text(raw, "message"))
    }

    /// An auto event's envelope `ts`.
    pub fn at(&self) -> Option<&str> {
        match self {
            Event::Auto { raw, .. } => raw.get("ts")?.as_str(),
            _ => None,
        }
    }

    /// `Some` only for `Event::TokenIndex`; a null or absent `changedAt` is `None`.
    pub fn token_index(&self) -> Option<TokenIndexPayload> {
        match self {
            Event::TokenIndex { raw, .. } => Some(parse_token_index(raw)),
            _ => None,
        }
    }

    fn auto_raw(&self, want: &str) -> Option<&Value> {
        match self {
            Event::Auto { kind, raw, .. } if kind == want => Some(raw),
            _ => None,
        }
    }
}

/// A `poll`'s active account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveAccount {
    pub number: u32,
    pub email: String,
}

/// A `poll` event's fields (§11.4), read as leniently as the TUI reads them: a key the daemon
/// left out is empty, never an error.
#[derive(Debug, Clone, PartialEq)]
pub struct PollPayload {
    pub active: Option<ActiveAccount>,
    /// Headroom by position; `None` where the daemon had no reading.
    pub headroom_pct: BTreeMap<u32, Option<f64>>,
    pub threshold: Option<f64>,
    pub fetch_errors: BTreeMap<u32, String>,
    /// Each position's windows and their usage, in the order the wire gives them.
    pub windows_pct: BTreeMap<u32, Vec<(String, f64)>>,
}

/// A window at the usage that bound an account at a poll. `used_pct` is what a reader can rely
/// on. `window` is the first window of that usage in the wire's key order, which on a tie is not
/// always the one auto counts (`rank::binding_window` under `autoswitch.models`): the wire does
/// not say which, and nothing reads `window` outside tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingWindow {
    pub window: String,
    pub used_pct: i64,
}

/// How near a window's usage must be to the account's own to be its binding window, in
/// percentage points. Strictly less than.
const BINDING_TOLERANCE: f64 = 0.05;

impl PollPayload {
    /// The usage that bound `position`: its relevant usage is 100 less its headroom, and
    /// `window` is the first of its `windows_pct`, in wire order, within the tolerance of that
    /// usage (see `BindingWindow`). `None` when the headroom is unknown or no window is that near.
    pub fn binding(&self, position: u32) -> Option<BindingWindow> {
        let headroom = (*self.headroom_pct.get(&position)?)?;
        let usage = 100.0 - headroom;
        let (window, _) = self
            .windows_pct
            .get(&position)?
            .iter()
            .find(|(_, pct)| (pct - usage).abs() < BINDING_TOLERANCE)?;
        Some(BindingWindow {
            window: window.clone(),
            used_pct: usage.round() as i64,
        })
    }
}

/// A `switch` event's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchPayload {
    pub trigger: Wire<Trigger>,
    pub from: u32,
    pub to: u32,
    pub warnings: Vec<String>,
    pub dry_run: bool,
}

/// A `no-switch` event's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoSwitchPayload {
    pub reason: Wire<NoSwitchReason>,
    pub detail: String,
}

/// An `all-exhausted` event's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllExhaustedPayload {
    pub earliest_reset_at: Option<String>,
}

/// The `token-index` fields the TUI reads (token report §12): when the index last changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenIndexPayload {
    pub changed_at: Option<String>,
}

/// A `token-index` object's fields; a null or absent `changedAt` is `None`.
pub fn parse_token_index(raw: &Value) -> TokenIndexPayload {
    TokenIndexPayload {
        changed_at: optional_text(raw, "changedAt"),
    }
}

/// `raw`'s string `key`, empty when it holds none.
fn text(raw: &Value, key: &str) -> String {
    optional_text(raw, key).unwrap_or_default()
}

/// `raw`'s string `key`; `None` when it is absent, null or not a string.
fn optional_text(raw: &Value, key: &str) -> Option<String> {
    raw.get(key)?.as_str().map(str::to_owned)
}

/// A `poll` object's fields. `None` only for a value that is not an object.
pub fn parse_poll(raw: &Value) -> Option<PollPayload> {
    let o = raw.as_object()?;
    let by_position = |key: &str| {
        o.get(key)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(position, v)| Some((position.parse::<u32>().ok()?, v)))
    };
    Some(PollPayload {
        active: o.get("active").and_then(Value::as_object).and_then(|a| {
            Some(ActiveAccount {
                number: u32::try_from(a.get("number")?.as_u64()?).ok()?,
                email: a
                    .get("email")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            })
        }),
        headroom_pct: by_position("headroomPct")
            .map(|(position, v)| (position, v.as_f64()))
            .collect(),
        threshold: o.get("threshold").and_then(Value::as_f64),
        fetch_errors: by_position("fetchErrors")
            .filter_map(|(position, v)| Some((position, v.as_str()?.to_owned())))
            .collect(),
        windows_pct: by_position("windowsPct")
            .filter_map(|(position, v)| {
                let windows = v
                    .as_object()?
                    .iter()
                    .filter_map(|(window, pct)| Some((window.clone(), pct.as_f64()?)))
                    .collect();
                Some((position, windows))
            })
            .collect(),
    })
}

/// A `switch` object's fields. `None` without a `from` and a `to`; a missing `trigger` reads as
/// empty, `warnings` as none and `dryRun` as false.
pub fn parse_switch(raw: &Value) -> Option<SwitchPayload> {
    let o = raw.as_object()?;
    let position = |key: &str| u32::try_from(o.get(key)?.as_u64()?).ok();
    Some(SwitchPayload {
        trigger: Wire::from_wire(o.get("trigger").and_then(Value::as_str).unwrap_or_default()),
        from: position("from")?,
        to: position("to")?,
        warnings: o
            .get("warnings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|w| Some(w.as_str()?.to_owned()))
            .collect(),
        dry_run: o.get("dryRun").and_then(Value::as_bool) == Some(true),
    })
}

/// A `no-switch` object's fields. `None` only for a value that is not an object; a missing
/// `reason` or `detail` reads as empty.
pub fn parse_no_switch(raw: &Value) -> Option<NoSwitchPayload> {
    let o = raw.as_object()?;
    let text = |key: &str| o.get(key).and_then(Value::as_str).unwrap_or_default();
    Some(NoSwitchPayload {
        reason: Wire::from_wire(text("reason")),
        detail: text("detail").to_owned(),
    })
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

/// The auto kinds that say an account's reading or state changed (D5.5).
const ACCOUNT_KINDS: [&str; 4] = [
    "poll",
    "switch",
    "account-quarantined",
    "account-unquarantined",
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
            mode: Wire::from_wire(o.get("mode")?.as_str()?),
            waiting_for: nullable_str(o.get("waitingFor"))?
                .as_deref()
                .map(Wire::from_wire),
            holder_pid: nullable_u32(o.get("holderPid"))?,
            backoff: match o.get("backoff") {
                None | Some(Value::Null) => None,
                Some(b) => Some(serde_json::from_value(b.clone()).ok()?),
            },
        },
        "stopping" => Event::Stopping {
            reason: Wire::from_wire(o.get("reason")?.as_str()?),
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
    use tagteam_cc::CLAUDE_CODE;
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
            // In no JSON: the event's lines below are the same with or without it.
            counting_window: BTreeMap::from([(1, "5h".into())]),
        };
        let quiet = AutoEvent::Poll {
            provider: cc(),
            active: None,
            headroom_pct: BTreeMap::new(),
            threshold: 90.0,
            fetch_errors: BTreeMap::new(),
            windows_pct: BTreeMap::new(),
            counting_window: BTreeMap::new(),
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
                    email: "sam@corp.example".into(),
                    reason: "invalid_grant".into(),
                },
                json!({"event": "account-quarantined", "number": 3, "email": "sam@corp.example",
                       "reason": "invalid_grant"}),
            ),
            (
                AutoEvent::AccountUnquarantined {
                    provider: cc(),
                    number: 3,
                    email: "sam@corp.example".into(),
                    reason: "account-replaced".into(),
                },
                json!({"event": "account-unquarantined", "number": 3, "email": "sam@corp.example",
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
                    // In no JSON: the line below is what it was before the field.
                    counting_window: BTreeMap::from([(1, "5h".into())]),
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
                    counting_window: BTreeMap::new(),
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
                    email: "sam@corp.example".into(),
                    reason: "invalid_grant".into(),
                },
                r#"{"schemaVersion":1,"event":"account-quarantined","ts":"2026-09-21T14:13:20Z","provider":"claude-code","number":3,"email":"sam@corp.example","reason":"invalid_grant"}"#,
            ),
            (
                AutoEvent::AccountUnquarantined {
                    provider: cc(),
                    number: 3,
                    email: "sam@corp.example".into(),
                    reason: "account-replaced".into(),
                },
                r#"{"schemaVersion":1,"event":"account-unquarantined","ts":"2026-09-21T14:13:20Z","provider":"claude-code","number":3,"email":"sam@corp.example","reason":"account-replaced"}"#,
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
    fn each_auto_payload_parses_from_event_json_s_own_output() {
        let mut seen = [false; 3];
        for (event, _) in auto_lines() {
            let raw = event_json(&event, T0);
            let typed = parse_event(&raw).unwrap();
            match &event {
                AutoEvent::Poll {
                    active,
                    headroom_pct,
                    threshold,
                    fetch_errors,
                    windows_pct,
                    ..
                } => {
                    seen[0] = true;
                    let want = PollPayload {
                        active: active.as_ref().map(|(number, email)| ActiveAccount {
                            number: *number,
                            email: email.clone(),
                        }),
                        headroom_pct: headroom_pct.clone(),
                        threshold: Some(*threshold),
                        fetch_errors: fetch_errors.clone(),
                        windows_pct: windows_pct
                            .iter()
                            .map(|(position, w)| {
                                (*position, w.iter().map(|(k, v)| (k.clone(), *v)).collect())
                            })
                            .collect(),
                    };
                    assert_eq!(parse_poll(&raw), Some(want.clone()));
                    assert_eq!(typed.poll(), Some(want));
                    assert_eq!(typed.switch(), None);
                    assert_eq!(typed.no_switch(), None);
                }
                AutoEvent::Switch {
                    trigger,
                    from,
                    to,
                    warnings,
                    dry_run,
                    ..
                } => {
                    seen[1] = true;
                    let want = SwitchPayload {
                        trigger: Wire::Known(*trigger),
                        from: *from,
                        to: *to,
                        warnings: warnings.clone(),
                        dry_run: *dry_run,
                    };
                    assert_eq!(parse_switch(&raw), Some(want.clone()));
                    assert_eq!(typed.switch(), Some(want));
                    assert_eq!(typed.poll(), None);
                    assert_eq!(typed.no_switch(), None);
                }
                AutoEvent::NoSwitch { reason, detail, .. } => {
                    seen[2] = true;
                    let want = NoSwitchPayload {
                        reason: Wire::from_wire(reason),
                        detail: detail.clone(),
                    };
                    assert_eq!(parse_no_switch(&raw), Some(want.clone()));
                    assert_eq!(typed.no_switch(), Some(want));
                    assert_eq!(typed.poll(), None);
                    assert_eq!(typed.switch(), None);
                }
                _ => {
                    assert_eq!(typed.poll(), None);
                    assert_eq!(typed.switch(), None);
                    assert_eq!(typed.no_switch(), None);
                }
            }
        }
        assert_eq!(seen, [true; 3]);
    }

    #[test]
    fn the_kinds_the_tui_reads_parse_from_event_json_s_own_output() {
        let mut seen = [false; 3];
        for (event, _) in auto_lines() {
            let raw = event_json(&event, T0);
            let typed = parse_event(&raw).unwrap();
            assert_eq!(typed.at(), Some("2026-09-21T14:13:20Z"), "{raw}");
            let changes = matches!(
                event,
                AutoEvent::Poll { .. }
                    | AutoEvent::Switch { .. }
                    | AutoEvent::AccountQuarantined { .. }
                    | AutoEvent::AccountUnquarantined { .. }
            );
            assert_eq!(typed.changes_an_account(), changes, "{raw}");
            let (mut exhausted, mut error, mut warning) = (None, None, None);
            match &event {
                AutoEvent::AllExhausted {
                    earliest_reset_at, ..
                } => {
                    seen[0] = true;
                    exhausted = Some(AllExhaustedPayload {
                        earliest_reset_at: earliest_reset_at.map(format_iso8601),
                    });
                }
                AutoEvent::Error { message, .. } => {
                    seen[1] = true;
                    error = Some(message.clone());
                }
                AutoEvent::ConfigWarning { message, .. } => {
                    seen[2] = true;
                    warning = Some(message.clone());
                }
                _ => {}
            }
            assert_eq!(typed.all_exhausted(), exhausted, "{raw}");
            assert_eq!(typed.error(), error, "{raw}");
            assert_eq!(typed.config_warning(), warning, "{raw}");
            assert_eq!(typed.token_index(), None, "{raw}");
        }
        assert_eq!(seen, [true; 3]);

        // A kind's object without its field reads as empty or none, never as `Unknown`.
        let bare = |kind: &str| parse_event(&json!({"event": kind, "provider": "p"})).unwrap();
        assert_eq!(
            bare("all-exhausted").all_exhausted(),
            Some(AllExhaustedPayload {
                earliest_reset_at: None
            })
        );
        assert_eq!(bare("error").error(), Some(String::new()));
        assert_eq!(bare("config-warning").config_warning(), Some(String::new()));
        assert_eq!(bare("error").at(), None, "no ts");
        assert!(bare("poll").changes_an_account());
        assert!(!bare("sleep").changes_an_account());

        // `token-index`'s `changedAt`, as the hub writes it; null or absent is none.
        let index = |changed_at_ms| {
            let state = IndexState {
                changed_at_ms,
                ..IndexState::default()
            };
            let v = daemon_event_json(
                &DaemonEvent::TokenIndex {
                    provider: cc(),
                    state,
                },
                T0,
            );
            parse_event(&v).unwrap()
        };
        let changed = index(Some((T0 - 60) * 1000));
        assert_eq!(
            changed.token_index(),
            Some(TokenIndexPayload {
                changed_at: Some("2026-09-21T14:12:20Z".into())
            })
        );
        assert_eq!(changed.at(), None, "not an auto kind");
        assert!(!changed.changes_an_account());
        let never = TokenIndexPayload { changed_at: None };
        assert_eq!(index(None).token_index(), Some(never.clone()));
        let keyless = json!({"event": "token-index", "provider": "p"});
        assert_eq!(parse_event(&keyless).unwrap().token_index(), Some(never));
        assert_eq!(parse_token_index(&json!({"changedAt": 5})).changed_at, None);
        assert_eq!(Event::Changed.token_index(), None);
        assert_eq!(Event::Changed.all_exhausted(), None);
        assert_eq!(Event::Changed.error(), None);
        assert_eq!(Event::Changed.config_warning(), None);
    }

    #[test]
    fn a_stop_reason_reads_back_as_the_daemon_spelled_it() {
        for reason in StopReason::ALL {
            let v = daemon_event_json(&DaemonEvent::Stopping { reason }, T0);
            assert_eq!(
                v.to_string(),
                format!(
                    r#"{{"schemaVersion":1,"event":"stopping","ts":"2026-09-21T14:13:20Z","reason":"{}"}}"#,
                    reason.as_str()
                ),
                "the wire is unchanged"
            );
            assert_eq!(
                parse_event(&v),
                Some(Event::Stopping {
                    reason: Wire::Known(reason)
                })
            );
        }
        let later = json!({"schemaVersion": 1, "event": "stopping", "ts": "x", "reason": "power"});
        let Some(Event::Stopping { reason }) = parse_event(&later) else {
            panic!("a reason a later daemon adds is still `stopping`");
        };
        assert_eq!(reason, Wire::Other("power".into()));
        assert_eq!(reason.as_str(), "power");
        // As before the retype: a `stopping` without its reason is a kind this client cannot read.
        let bare = json!({"schemaVersion": 1, "event": "stopping", "ts": "x"});
        assert_eq!(
            parse_event(&bare),
            Some(Event::Unknown {
                kind: "stopping".into(),
                raw: bare.clone(),
            })
        );
    }

    #[test]
    fn an_auto_payload_reads_as_leniently_as_the_tui_does() {
        let switch = parse_switch(&json!({"from": 2, "to": 1})).unwrap();
        assert_eq!(switch.trigger, Wire::Other(String::new()));
        assert!(switch.warnings.is_empty());
        assert!(!switch.dry_run);
        assert_eq!(parse_switch(&json!({"from": 2})), None);
        assert_eq!(parse_switch(&json!({"to": 1})), None);
        let newer = parse_switch(&json!({"trigger": "later", "from": 1, "to": 2})).unwrap();
        assert_eq!(newer.trigger, Wire::Other("later".into()));

        let bare = parse_no_switch(&json!({})).unwrap();
        assert_eq!(bare.reason, Wire::Other(String::new()));
        assert_eq!(bare.detail, "");
        let newer = parse_no_switch(&json!({"reason": "later", "detail": "d"})).unwrap();
        assert_eq!(newer.reason, Wire::Other("later".into()));
        assert_eq!(newer.reason.as_str(), "later");
        assert_eq!(newer.detail, "d");

        let poll = parse_poll(&json!({"headroomPct": {"1": null, "2": 30.5}})).unwrap();
        assert_eq!(poll.active, None);
        assert_eq!(
            poll.headroom_pct,
            BTreeMap::from([(1, None), (2, Some(30.5))])
        );
        assert_eq!(poll.threshold, None);
        assert!(poll.fetch_errors.is_empty());
        assert!(poll.windows_pct.is_empty());
        assert_eq!(parse_poll(&json!("poll")), None);
    }

    #[test]
    fn a_poll_reads_what_it_can_and_skips_the_rest() {
        let no_email = parse_poll(&json!({"active": {"number": 2}})).unwrap();
        assert_eq!(
            no_email.active,
            Some(ActiveAccount {
                number: 2,
                email: String::new()
            })
        );

        let text = parse_poll(&json!({"threshold": "90"})).unwrap();
        assert_eq!(text.threshold, None);

        let poll = parse_poll(&json!({
            "fetchErrors": {"1": "http-429", "x": "http-500", "2": 7, "3": "timeout"},
            "windowsPct": {
                "1": {"5h": 20.0, "7d": "high", "30d": 5.0},
                "y": {"5h": 1.0},
                "2": "none",
                "3": {"5h": 3.0}
            },
        }))
        .unwrap();
        assert_eq!(
            poll.fetch_errors,
            BTreeMap::from([(1, "http-429".to_owned()), (3, "timeout".to_owned())])
        );
        assert_eq!(
            poll.windows_pct,
            BTreeMap::from([
                (1, vec![("5h".to_owned(), 20.0), ("30d".to_owned(), 5.0)]),
                (3, vec![("5h".to_owned(), 3.0)]),
            ])
        );
    }

    #[test]
    fn the_binding_window_is_the_first_within_the_tolerance() {
        let poll = |headroom: Value, windows: Value| {
            let mut raw = json!({"event": "poll", "headroomPct": {"1": headroom}});
            if !windows.is_null() {
                raw["windowsPct"] = json!({"1": windows});
            }
            parse_poll(&raw).unwrap()
        };
        let bound = |window: &str, used_pct| {
            Some(BindingWindow {
                window: window.into(),
                used_pct,
            })
        };

        // Two windows within the tolerance: the first in the wire's key order, not the nearer.
        let both = poll(json!(60.0), json!({"7d": 40.04, "5h": 40.0}));
        assert_eq!(both.binding(1), bound("7d", 40));
        let both = poll(json!(60.0), json!({"5h": 40.0, "7d": 40.04}));
        assert_eq!(both.binding(1), bound("5h", 40));

        // 0.049 away matches, 0.051 does not, on either side.
        assert_eq!(
            poll(json!(60.0), json!({"5h": 40.049})).binding(1),
            bound("5h", 40)
        );
        assert_eq!(
            poll(json!(60.0), json!({"5h": 39.951})).binding(1),
            bound("5h", 40)
        );
        assert_eq!(poll(json!(60.0), json!({"5h": 40.051})).binding(1), None);
        assert_eq!(poll(json!(60.0), json!({"5h": 39.949})).binding(1), None);

        // A window further off is skipped for one that matches.
        let skipped = poll(json!(60.0), json!({"5h": 10.0, "7d": 40.0}));
        assert_eq!(skipped.binding(1), bound("7d", 40));

        // The used percentage is the usage, rounded.
        assert_eq!(
            poll(json!(59.6), json!({"5h": 40.4})).binding(1),
            bound("5h", 40)
        );
        assert_eq!(
            poll(json!(59.4), json!({"5h": 40.6})).binding(1),
            bound("5h", 41)
        );

        // An unknown headroom, a missing windowsPct, and another position bind nothing.
        assert_eq!(poll(Value::Null, json!({"5h": 40.0})).binding(1), None);
        assert_eq!(poll(json!(60.0), Value::Null).binding(1), None);
        assert_eq!(poll(json!(60.0), json!({"5h": 40.0})).binding(2), None);
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
        // The client reads a reply's id through a `Value`, and so does this.
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
                events: vec![
                    Wire::Known(NotifyEvent::Switch),
                    Wire::Known(NotifyEvent::Quarantine),
                    Wire::Known(NotifyEvent::Limit),
                ],
            }
        );
        assert_eq!(
            status.providers[0],
            StatusProvider {
                provider: "claude-code".into(),
                mode: Wire::from_wire("auto"),
                autoswitch: true,
                waiting_for: None,
                holder_pid: None,
                backoff: None,
                last_tick_at: Some(iso(T + 60)),
                next_tick_at: Some(iso(T + 360)),
                last_outcome: Some(StatusOutcome {
                    kind: Wire::from_wire("no-switch"),
                    reason: Some("below-threshold".into()),
                    at: iso(T + 60),
                }),
                last_switch: Some(StatusSwitch {
                    at: iso(T + 30),
                    from: 1,
                    to: 2,
                    trigger: Wire::from_wire("proactive"),
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
            status.providers[2].waiting_for.as_ref().map(Wire::as_str),
            Some("engine-lock")
        );
        assert_eq!(status.providers[2].holder_pid, Some(5151));
        assert_eq!(
            status.providers[3].waiting_for.as_ref().map(Wire::as_str),
            Some("schema")
        );
        assert_eq!(status.providers[4].mode.as_str(), "idle");
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
    fn a_status_reads_its_typed_fields_as_the_daemon_wrote_them() {
        let v = status_json(&snapshot(), Path::new(HOME));
        let status: Status = serde_json::from_value(v.clone()).unwrap();
        let auto = &status.providers[0];
        assert_eq!(auto.mode, Wire::Known(Mode::Auto));
        assert_eq!(auto.waiting_for, None);
        assert_eq!(
            auto.last_outcome.as_ref().map(|o| &o.kind),
            Some(&Wire::Known(OutcomeKind::NoSwitch))
        );
        let waiting = &status.providers[2];
        assert_eq!(waiting.mode, Wire::Known(Mode::Waiting));
        assert_eq!(
            waiting.waiting_for,
            Some(Wire::Known(WaitingFor::EngineLock))
        );
        assert_eq!(
            status.providers[3].waiting_for,
            Some(Wire::Known(WaitingFor::Schema))
        );
        assert_eq!(
            status.providers[4].last_outcome.as_ref().map(|o| &o.kind),
            Some(&Wire::Known(OutcomeKind::Error))
        );
        assert_eq!(
            auto.last_switch.as_ref().map(|w| &w.trigger),
            Some(&Wire::Known(Trigger::Proactive))
        );
        assert_eq!(
            status.notifications.events,
            NotifyEvent::ALL.map(Wire::Known).to_vec()
        );
        // An optional field left out reads as none, as a null does.
        let mut absent = v.clone();
        absent["providers"][2]
            .as_object_mut()
            .unwrap()
            .remove("waitingFor");
        let absent: Status = serde_json::from_value(absent).unwrap();
        assert_eq!(absent.providers[2].waiting_for, None);
        // D5.6: a spelling a later daemon writes is kept as it came.
        let mut later = v;
        later["providers"][2]["mode"] = json!("paused");
        later["providers"][2]["waitingFor"] = json!("later");
        later["providers"][4]["lastOutcome"]["kind"] = json!("rested");
        later["providers"][0]["lastSwitch"]["trigger"] = json!("added-later");
        later["notifications"]["events"] = json!(["switch", "digest"]);
        let later: Status = serde_json::from_value(later).unwrap();
        let paused = &later.providers[2];
        assert_eq!(paused.mode, Wire::Other("paused".into()));
        assert_eq!(paused.mode.as_str(), "paused");
        assert_eq!(paused.waiting_for, Some(Wire::Other("later".into())));
        assert!(!paused.collects_for_itself());
        assert!(!paused.waits_for_upgrade());
        assert_eq!(
            later.providers[4].last_outcome.as_ref().map(|o| &o.kind),
            Some(&Wire::Other("rested".into()))
        );
        let trigger = later.providers[0].last_switch.as_ref().map(|w| &w.trigger);
        assert_eq!(trigger, Some(&Wire::Other("added-later".into())));
        assert_eq!(
            later.notifications.events,
            [
                Wire::Known(NotifyEvent::Switch),
                Wire::Other("digest".into())
            ]
        );
        let mut not_a_string = status_json(&snapshot(), Path::new(HOME));
        not_a_string["providers"][0]["mode"] = json!(7);
        assert!(serde_json::from_value::<Status>(not_a_string).is_err());
    }

    #[test]
    fn every_daemon_kind_reads_back_from_daemon_event_json() {
        // D5.5's daemon kinds, through the function the hub writes them with.
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
                    mode: Wire::from_wire("waiting"),
                    waiting_for: Some(Wire::from_wire("engine-lock")),
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
                    mode: Wire::from_wire("collect"),
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
                    reason: Wire::from_wire("upgrade"),
                },
            ),
            (
                DaemonEvent::Stopping {
                    reason: StopReason::Schema,
                },
                Event::Stopping {
                    reason: Wire::from_wire("schema"),
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
                mode: Wire::from_wire("auto"),
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

    fn status_provider(mode: &str, waiting_for: Option<&str>) -> StatusProvider {
        StatusProvider {
            provider: "claude-code".into(),
            mode: Wire::from_wire(mode),
            autoswitch: true,
            waiting_for: waiting_for.map(Wire::from_wire),
            holder_pid: None,
            backoff: None,
            last_tick_at: None,
            next_tick_at: None,
            last_outcome: None,
            last_switch: None,
        }
    }

    #[test]
    fn a_wire_value_keeps_an_unknown_spelling() {
        let other = Wire::<Mode>::from_wire("paused");
        assert_eq!(other, Wire::Other("paused".into()));
        assert_eq!(other.as_str(), "paused");
        assert_eq!(other.known(), None);
        let known = Wire::<Mode>::from_wire("auto");
        assert_eq!(known, Wire::Known(Mode::Auto));
        assert_eq!(known.as_str(), "auto");
        assert_eq!(known.known(), Some(Mode::Auto));
        assert_eq!(
            Wire::<Mode>::from_wire("Auto"),
            Wire::Other("Auto".into()),
            "a spelling is exact"
        );
        for reason in NoSwitchReason::ALL {
            assert_eq!(
                Wire::<NoSwitchReason>::from_wire(reason.as_str()),
                Wire::Known(reason)
            );
        }
        assert_eq!(
            Wire::<Trigger>::from_wire("at-limit"),
            Wire::Known(Trigger::AtLimit)
        );
    }

    #[test]
    fn the_mode_predicates_default_to_false_for_unknown_values() {
        // (mode, waitingFor, collects_for_itself, waits_for_upgrade)
        let table = [
            ("auto", None, true, false),
            ("auto", Some("engine-lock"), true, false),
            ("auto", Some("schema"), true, false),
            ("auto", Some("later"), true, false),
            ("collect", None, true, false),
            ("collect", Some("engine-lock"), true, false),
            ("collect", Some("schema"), true, false),
            ("collect", Some("later"), true, false),
            ("waiting", Some("engine-lock"), true, false),
            ("waiting", Some("schema"), false, true),
            ("waiting", Some("later"), false, false),
            ("waiting", None, false, false),
            ("idle", None, false, false),
            ("idle", Some("engine-lock"), false, false),
            ("idle", Some("schema"), false, false),
            ("idle", Some("later"), false, false),
            ("paused", None, false, false),
            ("paused", Some("engine-lock"), false, false),
            ("paused", Some("schema"), false, false),
            ("paused", Some("later"), false, false),
        ];
        for (mode, waiting_for, collects, upgrade) in table {
            let p = status_provider(mode, waiting_for);
            assert_eq!(p.collects_for_itself(), collects, "{mode} {waiting_for:?}");
            assert_eq!(p.waits_for_upgrade(), upgrade, "{mode} {waiting_for:?}");
        }
    }

    #[test]
    fn the_typed_accessors_read_the_strings_a_daemon_wrote() {
        let p = status_provider("waiting", Some("engine-lock"));
        assert_eq!(p.mode, Wire::Known(Mode::Waiting));
        assert_eq!(p.waiting_for, Some(Wire::Known(WaitingFor::EngineLock)));
        let p = status_provider("paused", Some("later"));
        assert_eq!(p.mode, Wire::Other("paused".into()));
        assert_eq!(p.waiting_for, Some(Wire::Other("later".into())));
        assert_eq!(status_provider("auto", None).waiting_for, None);

        let outcome = |kind: &str| StatusOutcome {
            kind: Wire::from_wire(kind),
            reason: None,
            at: iso(T),
        };
        assert_eq!(
            outcome("no-switch").kind,
            Wire::Known(OutcomeKind::NoSwitch)
        );
        assert_eq!(outcome("rested").kind, Wire::Other("rested".into()));

        let mode_event = |mode: &str, waiting_for: Option<&str>| Event::Mode {
            provider: "claude-code".into(),
            mode: Wire::from_wire(mode),
            waiting_for: waiting_for.map(Wire::from_wire),
            holder_pid: None,
            backoff: None,
        };
        let e = mode_event("waiting", Some("schema"));
        assert_eq!(e.mode(), Some(Wire::Known(Mode::Waiting)));
        assert_eq!(e.waiting_for(), Some(Wire::Known(WaitingFor::Schema)));
        let e = mode_event("paused", None);
        assert_eq!(e.mode(), Some(Wire::Other("paused".into())));
        assert_eq!(e.waiting_for(), None);
        assert_eq!(Event::Changed.mode(), None);
        assert_eq!(Event::Changed.waiting_for(), None);
        let stopping = Event::Stopping {
            reason: Wire::from_wire("signal"),
        };
        assert_eq!(stopping.mode(), None);
        assert_eq!(stopping.waiting_for(), None);
    }
}
