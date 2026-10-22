//! Claude Code's session records (Appendix A.7) as the neutral liveness judgement reads them
//! (refactor spec §9).

use tagteam_provider::liveness::proc_start_of;
use tagteam_provider::{
    FakeProcess, FakeProcessProbe, ProcStart, parse_session_record, record_is_live,
};

/// `"Thu Oct  1 12:34:56 2026"` as epoch seconds.
const LSTART_S: i64 = 1_790_858_096;
const LSTART: &str = "Thu Oct  1 12:34:56 2026";
/// The record's `startedAt`, epoch ms: the same second.
const STARTED_AT_MS: i64 = LSTART_S * 1000;

/// Whether the record `{"pid": 100, "startedAt": …, <proc_start>}` is live against a process with
/// the given facts. `proc_start` is a raw JSON member (`"procStart": …`), or empty for none.
fn live(proc_start: &str, process: FakeProcess) -> bool {
    let member = if proc_start.is_empty() {
        String::new()
    } else {
        format!(", {proc_start}")
    };
    let bytes = format!(r#"{{"pid": 100, "startedAt": {STARTED_AT_MS}{member}}}"#);
    let record = parse_session_record(bytes.as_bytes()).expect("a readable record");
    let probe = FakeProcessProbe::new();
    probe.set(100, process);
    record_is_live(&probe, &record, "claude")
}

fn process(
    exists: Option<bool>,
    start_time_s: Option<i64>,
    start_ticks: Option<u64>,
    mentions_launch: Option<bool>,
) -> FakeProcess {
    FakeProcess {
        exists,
        start_time_s,
        start_ticks,
        mentions_launch,
        ppid: None,
    }
}

fn lstart_member() -> String {
    format!(r#""procStart": "{LSTART}""#)
}

#[test]
fn each_proc_start_form_keeps_its_liveness_outcome() {
    let lstart = lstart_member();
    let at =
        |start: Option<i64>, mentions: Option<bool>| process(Some(true), start, None, mentions);

    // An lstart: within a second is live; otherwise recycled only when nothing mentions the
    // launch command (Decision 16).
    assert!(live(&lstart, at(Some(LSTART_S + 1), Some(false))));
    assert!(!live(&lstart, at(Some(LSTART_S + 2), Some(false))));
    assert!(live(&lstart, at(Some(LSTART_S + 2), None)));
    assert!(live(&lstart, at(None, Some(false))));

    // Boot ticks, as text or as a number.
    for member in [r#""procStart": "5000""#, r#""procStart": 5000"#] {
        for (ticks, expected) in [(Some(5_000), true), (Some(5_001), false), (None, true)] {
            assert_eq!(
                live(member, process(Some(true), None, ticks, None)),
                expected,
                "{member}, ticks {ticks:?}"
            );
        }
    }

    // Boot ticks too large for a u64 cannot be compared, so they are live.
    for member in [
        r#""procStart": "99999999999999999999""#,
        r#""procStart": 99999999999999999999"#,
    ] {
        for ticks in [Some(5_000), None] {
            assert!(
                live(member, process(Some(true), None, ticks, None)),
                "{member}, ticks {ticks:?}"
            );
        }
    }

    // No usable start: the 120 s `startedAt` heuristic decides.
    let unusable = [
        "",
        r#""procStart": null"#,
        r#""procStart": """#,
        r#""procStart": "not a date""#,
        r#""procStart": "-5""#,
        r#""procStart": -5"#,
    ];
    for member in unusable {
        assert!(
            !live(member, at(Some(LSTART_S + 121), Some(false))),
            "{member}"
        );
        assert!(
            live(member, at(Some(LSTART_S + 120), Some(false))),
            "{member}"
        );
        assert!(
            live(member, at(Some(LSTART_S + 121), Some(true))),
            "{member}"
        );
    }

    // A pid that does not exist is gone, and one that cannot be probed is live, whatever the form.
    let every = unusable.iter().copied().chain([
        lstart.as_str(),
        r#""procStart": "5000""#,
        r#""procStart": 5000"#,
        r#""procStart": "99999999999999999999""#,
        r#""procStart": 99999999999999999999"#,
    ]);
    for member in every {
        assert!(
            !live(member, process(Some(false), None, None, None)),
            "{member}"
        );
        assert!(live(member, process(None, None, None, None)), "{member}");
    }
}

#[test]
fn each_proc_start_text_maps_to_its_form() {
    let cases: [(&[Option<&str>], ProcStart); 5] = [
        (
            &[
                Some("Thu Oct  1 12:34:56 2026"),
                Some(" Thu Oct  1 12:34:56 2026\n"),
            ],
            ProcStart::EpochS(LSTART_S),
        ),
        (
            &[Some("5000"), Some(" 5000\n")],
            ProcStart::BootTicks(5_000),
        ),
        (
            &[Some("18446744073709551615")],
            ProcStart::BootTicks(u64::MAX),
        ),
        (
            &[
                Some("18446744073709551616"),
                Some("99999999999999999999999999"),
            ],
            ProcStart::BootTicksIndeterminate,
        ),
        (
            &[
                None,
                Some(""),
                Some("  "),
                Some("not a date"),
                Some("-5"),
                Some("1.5"),
                Some("Thu Oct  1 12:34:56 1969"),
            ],
            ProcStart::Unknown,
        ),
    ];
    for (texts, form) in cases {
        for text in texts {
            assert_eq!(proc_start_of(*text), form, "{text:?}");
        }
    }
}
