//! Claude Code's session records (Appendix A.7): their format, and the neutral record each becomes.

use serde_json::Value;
use tagteam_provider::liveness::{ProcStart, SessionRecord};

/// One `<config_home>/sessions/<pid>.json` (Appendix A.7), as Claude Code writes it, before
/// `to_session_record` reduces it to what liveness and the token report's sightings need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcRecord {
    pub pid: u32,
    /// `ps -o lstart=` text, or the all-digit legacy form; `None` when absent or not text.
    pub proc_start: Option<String>,
    /// `startedAt`, epoch ms.
    pub started_at_ms: Option<i64>,
    pub kind: Option<String>,
    /// `sessionId`; `None` when absent or not text (token report spec §2.4).
    pub session_id: Option<String>,
}

/// A record nested deeper than this is malformed (§12.6 "deep nesting").
const MAX_DEPTH: usize = 32;

fn depth(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// `pid` as a process id `kill` may be given: 1 through `i32::MAX`. Zero and negative values
/// would address a process group, so they are out of range, never a pid to probe.
fn pid_of(v: Option<&Value>) -> Result<u32, String> {
    let Some(v) = v else {
        return Err("it has no pid".into());
    };
    let Value::Number(n) = v else {
        return Err("its pid is not an integer".into());
    };
    if let Some(p) = n.as_u64() {
        return u32::try_from(p)
            .ok()
            .filter(|p| (1..=i32::MAX as u32).contains(p))
            .ok_or_else(|| "its pid is out of range".to_owned());
    }
    let text = n.to_string();
    let integral = text
        .strip_prefix('-')
        .unwrap_or(&text)
        .bytes()
        .all(|b| b.is_ascii_digit());
    if integral {
        Err("its pid is out of range".into())
    } else {
        Err("its pid is not an integer".into())
    }
}

/// §12.6: non-object JSON, a missing or non-integer or out-of-range `pid`, depth > 32, or
/// invalid UTF-8 is `Err(detail)`; unknown fields are ignored. `detail` never quotes the bytes.
pub fn parse_cc_record(bytes: &[u8]) -> Result<CcRecord, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "it is not UTF-8".to_owned())?;
    let v: Value = serde_json::from_str(text).map_err(|_| "it is not JSON".to_owned())?;
    let Value::Object(o) = &v else {
        return Err("it is not a JSON object".into());
    };
    if depth(&v) > MAX_DEPTH {
        return Err(format!("it is nested deeper than {MAX_DEPTH} levels"));
    }
    let pid = pid_of(o.get("pid"))?;
    let proc_start = match o.get("procStart") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    Ok(CcRecord {
        pid,
        proc_start,
        started_at_ms: o.get("startedAt").and_then(Value::as_i64),
        kind: o.get("kind").and_then(Value::as_str).map(str::to_owned),
        session_id: o
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// The `kind`s of a session record written by a Claude Code background daemon or its workers
/// (Appendix A.7).
const DAEMON_KINDS: [&str; 3] = ["bg", "daemon", "daemon-worker"];

impl CcRecord {
    /// `start` is `proc_start_of(proc_start)`; `background` is whether `kind` is exactly one of
    /// `DAEMON_KINDS`; `pid`, `started_at_ms` and `session_id` are carried over.
    pub fn to_session_record(&self) -> SessionRecord {
        SessionRecord {
            pid: self.pid,
            start: proc_start_of(self.proc_start.as_deref()),
            started_at_ms: self.started_at_ms,
            session_id: self.session_id.clone(),
            background: self
                .kind
                .as_deref()
                .is_some_and(|k| DAEMON_KINDS.contains(&k)),
        }
    }
}

/// Claude Code's record, read as a neutral `SessionRecord`: `parse_cc_record`, then
/// `to_session_record`.
pub fn parse_session_record(bytes: &[u8]) -> Result<SessionRecord, String> {
    parse_cc_record(bytes).map(|r| r.to_session_record())
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `s` as a number of `min..=max` ASCII digits; no sign, no spaces.
fn digits(s: &str, min: usize, max: usize) -> Option<u32> {
    let ok = (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| s.parse().ok()).flatten()
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to the proleptic Gregorian date `y-m-d` (H. Hinnant's
/// `days_from_civil`). `m` is 1–12 and `d` is valid for the month.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `ps -o lstart=` with `LC_ALL=C TZ=UTC` (`Thu Oct  1 12:34:56 2026`) to epoch seconds. Runs of
/// spaces separate the fields, as `ps` pads a single-digit day. The weekday name must be one of
/// the seven but is not checked against the date. `None` for anything else.
pub fn parse_lstart(s: &str) -> Option<i64> {
    let fields: Vec<&str> = s.split_whitespace().collect();
    let [weekday, month, day, time, year] = fields.as_slice() else {
        return None;
    };
    if !WEEKDAYS.contains(weekday) {
        return None;
    }
    let month = MONTHS.iter().position(|m| m == month)? as u32 + 1;
    let year = i64::from(digits(year, 4, 4)?);
    if year < 1970 {
        return None;
    }
    let day = digits(day, 1, 2)?;
    if day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let hms: Vec<&str> = time.split(':').collect();
    let [h, m, sec] = hms.as_slice() else {
        return None;
    };
    let (h, m, sec) = (digits(h, 2, 2)?, digits(m, 2, 2)?, digits(sec, 2, 2)?);
    if h > 23 || m > 59 || sec > 59 {
        return None;
    }
    Some(
        days_from_civil(year, month, day) * 86_400
            + i64::from(h) * 3_600
            + i64::from(m) * 60
            + i64::from(sec),
    )
}

/// Claude Code's `procStart`, trimmed: `lstart` text is `EpochS`; otherwise non-empty all-digit
/// text is `BootTicks`, or `BootTicksIndeterminate` when it overflows `u64`; anything else,
/// absence included, is `Unknown`.
pub fn proc_start_of(text: Option<&str>) -> ProcStart {
    let text = text.map(str::trim).unwrap_or("");
    if let Some(epoch_s) = parse_lstart(text) {
        return ProcStart::EpochS(epoch_s);
    }
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return text
            .parse::<u64>()
            .map_or(ProcStart::BootTicksIndeterminate, ProcStart::BootTicks);
    }
    ProcStart::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tagteam_provider::liveness::{ProcessProbe, SystemProcessProbe};

    fn record(v: Value) -> Result<CcRecord, String> {
        parse_cc_record(v.to_string().as_bytes())
    }

    #[test]
    fn a_record_keeps_what_liveness_needs_and_ignores_the_rest() {
        let r = record(json!({
            "pid": 4242, "sessionId": "s", "cwd": "/w", "startedAt": 1_790_858_096_000_i64,
            "procStart": "Thu Oct  1 12:34:56 2026", "version": "2.1.286", "kind": "bg",
            "entrypoint": "cli", "status": "idle", "updatedAt": 1
        }))
        .unwrap();
        assert_eq!(
            r,
            CcRecord {
                pid: 4242,
                proc_start: Some("Thu Oct  1 12:34:56 2026".into()),
                started_at_ms: Some(1_790_858_096_000),
                kind: Some("bg".into()),
                session_id: Some("s".into()),
            }
        );
        assert_eq!(
            record(json!({"pid": 7})).unwrap(),
            CcRecord {
                pid: 7,
                proc_start: None,
                started_at_ms: None,
                kind: None,
                session_id: None,
            }
        );
    }

    #[test]
    fn a_numeric_proc_start_is_the_all_digit_legacy_form_and_any_other_shape_is_absent() {
        assert_eq!(
            record(json!({"pid": 7, "procStart": 123456}))
                .unwrap()
                .proc_start
                .as_deref(),
            Some("123456")
        );
        for odd in [json!(null), json!(true), json!({"t": 1}), json!([1])] {
            assert_eq!(
                record(json!({"pid": 7, "procStart": odd}))
                    .unwrap()
                    .proc_start,
                None
            );
        }
        assert_eq!(
            record(json!({"pid": 7, "startedAt": "soon", "kind": 3})).unwrap(),
            CcRecord {
                pid: 7,
                proc_start: None,
                started_at_ms: None,
                kind: None,
                session_id: None,
            }
        );
    }

    #[test]
    fn a_record_names_its_session_only_in_text() {
        // Token report spec §2.4: a record names its session; anything but text is no id.
        let id = |v: Value| record(v).unwrap().session_id;
        assert_eq!(
            id(json!({"pid": 7, "sessionId": "0193-abc"})).as_deref(),
            Some("0193-abc")
        );
        assert_eq!(id(json!({"pid": 7})), None);
        for odd in [
            json!(null),
            json!(7),
            json!(true),
            json!({"id": "s"}),
            json!(["s"]),
        ] {
            assert_eq!(id(json!({"pid": 7, "sessionId": odd})), None);
        }
    }

    #[test]
    fn a_malformed_record_is_an_error_that_never_quotes_its_bytes() {
        let nested = |levels: usize| {
            let mut s = String::from("{\"pid\": 7, \"x\": ");
            s.push_str(&"[".repeat(levels));
            s.push_str(&"]".repeat(levels));
            s.push('}');
            s.into_bytes()
        };
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("not JSON", b"SENTINEL{".to_vec()),
            ("an array", br#"["SENTINEL"]"#.to_vec()),
            ("a string", br#""SENTINEL""#.to_vec()),
            ("a number", b"7".to_vec()),
            ("no pid", br#"{"kind": "SENTINEL"}"#.to_vec()),
            ("a string pid", br#"{"pid": "SENTINEL"}"#.to_vec()),
            ("a fractional pid", br#"{"pid": 1.5}"#.to_vec()),
            ("pid 0", br#"{"pid": 0}"#.to_vec()),
            ("a negative pid", br#"{"pid": -1}"#.to_vec()),
            ("a pid above i32::MAX", br#"{"pid": 2147483648}"#.to_vec()),
            (
                "a huge pid",
                br#"{"pid": 99999999999999999999999999}"#.to_vec(),
            ),
            ("33 levels", nested(32)),
            ("200 levels", nested(199)),
            (
                "invalid UTF-8",
                b"{\"pid\": 7, \"k\": \"\xff\xfeSENTINEL\"}".to_vec(),
            ),
        ];
        for (case, bytes) in cases {
            let detail = parse_cc_record(&bytes).expect_err(case);
            assert!(!detail.contains("SENTINEL"), "{case}: {detail}");
        }
        assert_eq!(
            parse_cc_record(&nested(31)).unwrap().pid,
            7,
            "32 levels is the limit, not past it"
        );
        assert_eq!(
            record(json!({"pid": i32::MAX})).unwrap().pid,
            i32::MAX as u32
        );
    }

    #[test]
    fn lstart_text_parses_to_epoch_seconds() {
        for (text, epoch) in [
            ("Thu Jan  1 00:00:00 1970", 0),
            ("Thu Oct  1 12:34:56 2026", 1_790_858_096),
            ("Thu Oct 10 01:02:03 2030", 1_917_824_523),
            ("Fri Dec 31 23:59:59 1999", 946_684_799),
            ("Thu Feb 29 23:59:59 2024", 1_709_251_199),
            ("Tue Feb 29 00:00:00 2000", 951_782_400),
            ("Mon Mar  1 00:00:00 2100", 4_107_542_400),
            ("Thu Oct  1 12:34:56 2026\n", 1_790_858_096),
            ("  Thu Oct  1 12:34:56 2026  ", 1_790_858_096),
            // The weekday is not checked against the date: 2026-10-01 is a Thursday.
            ("Wed Oct  1 12:34:56 2026", 1_790_858_096),
        ] {
            assert_eq!(parse_lstart(text), Some(epoch), "{text:?}");
        }
    }

    #[test]
    fn anything_but_lstart_text_is_refused() {
        for text in [
            "",
            "garbage",
            "1790858096",
            "Mon Feb 29 00:00:00 2100",
            "Wed Feb 29 00:00:00 2023",
            "Thu Apr 31 00:00:00 2026",
            "Thu Oct  0 12:34:56 2026",
            "Thu Oct 32 12:34:56 2026",
            "Thu Oct +1 12:34:56 2026",
            "Thu Oct 001 12:34:56 2026",
            "Thu Foo  1 12:34:56 2026",
            "Xyz Oct  1 12:34:56 2026",
            "thu oct  1 12:34:56 2026",
            "Thu Oct  1 24:00:00 2026",
            "Thu Oct  1 12:60:00 2026",
            "Thu Oct  1 12:34:60 2026",
            "Thu Oct  1 12:34 2026",
            "Thu Oct  1 1:02:03 2026",
            "Thu Oct  1 12:34:56 1969",
            "Thu Oct  1 12:34:56 -2026",
            "Thu Oct  1 12:34:56 26",
            "Thu Oct  1 12:34:56 2026 UTC",
        ] {
            assert_eq!(parse_lstart(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_civil_arithmetic_agrees_with_every_day_from_1970_to_2400() {
        let mut expected = 0;
        for y in 1970..2400 {
            for m in 1..=12 {
                for d in 1..=days_in_month(y, m) {
                    assert_eq!(days_from_civil(y, m, d), expected, "{y}-{m}-{d}");
                    expected += 1;
                }
            }
        }
    }

    /// The `lstart` text for an epoch second, for the real-process test below.
    fn lstart_of(epoch_s: i64) -> String {
        let (days, secs) = (epoch_s.div_euclid(86_400), epoch_s.rem_euclid(86_400));
        let (mut y, mut d) = (1970, days);
        while d >= if is_leap(y) { 366 } else { 365 } {
            d -= if is_leap(y) { 366 } else { 365 };
            y += 1;
        }
        let mut m = 1;
        while d >= i64::from(days_in_month(y, m)) {
            d -= i64::from(days_in_month(y, m));
            m += 1;
        }
        format!(
            "{} {} {:>2} {:02}:{:02}:{:02} {y}",
            WEEKDAYS[(days + 4).rem_euclid(7) as usize],
            MONTHS[m as usize - 1],
            d + 1,
            secs / 3_600,
            secs / 60 % 60,
            secs % 60
        )
    }

    #[test]
    fn this_process_s_start_reads_back_from_its_lstart_text() {
        let start = SystemProcessProbe
            .start_time_s(std::process::id())
            .expect("this process's start time");
        assert_eq!(parse_lstart(&lstart_of(start)), Some(start));
    }
}
