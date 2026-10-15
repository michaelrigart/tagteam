//! Claude Code's transcripts (token report spec §2, §3.2): discovery and the usage line parser,
//! over synthetic lines with no real content.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use serde_json::{Value, json};
use tagteam_cc::transcripts::{discover, parse_usage_line};
use tagteam_core::tokens::TokenCounts;
use tagteam_provider::{LineError, UsageLine};

/// 2026-10-14T09:00:02.123Z.
const AT_MS: i64 = 1_791_968_402_123;

/// A final assistant line as Claude Code 2.1.299 writes one (spec §2.1), every count distinct.
fn final_line() -> Value {
    json!({
        "parentUuid": "u1", "isSidechain": false, "userType": "external", "cwd": "/w/app",
        "sessionId": "s1", "version": "2.1.299", "gitBranch": "main",
        "message": {
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
            "content": [{"type": "text", "text": "x"}],
            "stop_reason": "end_turn", "stop_sequence": null,
            "usage": {
                "input_tokens": 10, "cache_creation_input_tokens": 300,
                "cache_read_input_tokens": 2000,
                "cache_creation": {"ephemeral_5m_input_tokens": 200, "ephemeral_1h_input_tokens": 100},
                "output_tokens": 120, "service_tier": "standard",
                "server_tool_use": {"web_search_requests": 2, "web_fetch_requests": 0},
                "speed": "standard",
                "output_tokens_details": {"thinking_tokens": 30},
                "iterations": [{
                    "input_tokens": 10, "output_tokens": 120, "cache_read_input_tokens": 2000,
                    "cache_creation_input_tokens": 300,
                    "cache_creation": {"ephemeral_5m_input_tokens": 200, "ephemeral_1h_input_tokens": 100},
                    "type": "message"
                }]
            }
        },
        "requestId": "req_1", "type": "assistant", "uuid": "a1",
        "timestamp": "2026-10-14T09:00:02.123Z"
    })
}

fn parse(v: &Value) -> Result<Option<UsageLine>, LineError> {
    parse_usage_line(&serde_json::to_vec(v).unwrap())
}

fn parsed(v: &Value) -> UsageLine {
    parse(v).unwrap().expect("a usage line")
}

fn counts(input: u64, output: u64, m5: u64, h1: u64, read: u64, searches: u64) -> TokenCounts {
    TokenCounts {
        input,
        output,
        cache_write_5m: m5,
        cache_write_1h: h1,
        cache_read: read,
        web_search: searches,
    }
}

/// Removes the field at `path` (object keys, `/`-separated).
fn without(mut v: Value, path: &str) -> Value {
    let (parent, key) = path.rsplit_once('/').unwrap_or(("", path));
    let mut at = &mut v;
    for k in parent.split('/').filter(|k| !k.is_empty()) {
        at = &mut at[k];
    }
    at.as_object_mut().unwrap().remove(key);
    v
}

/// Sets the field at `path` to `to`.
fn with(mut v: Value, path: &str, to: Value) -> Value {
    let mut at = &mut v;
    for k in path.split('/') {
        at = &mut at[k];
    }
    *at = to;
    v
}

#[test]
fn parses_a_final_assistant_line() {
    assert_eq!(
        parsed(&final_line()),
        UsageLine {
            message_id: "msg_1".into(),
            request_id: "req_1".into(),
            session_id: "s1".into(),
            at_ms: AT_MS,
            model: "claude-opus-5-5".into(),
            project: Some("/w/app".into()),
            sidechain: false,
            counts: counts(10, 120, 200, 100, 2000, 2),
            speed: Some("standard".into()),
            final_line: true,
        }
    );
}

#[test]
fn a_stale_snapshot_is_not_final() {
    // Spec §2.2: a line with a null stop_reason holds a stale output count, and no iterations,
    // speed or server tool use.
    let mut stale = with(final_line(), "message/stop_reason", Value::Null);
    stale = with(stale, "message/usage/output_tokens", json!(5));
    for gone in [
        "iterations",
        "speed",
        "server_tool_use",
        "output_tokens_details",
    ] {
        stale = without(stale, &format!("message/usage/{gone}"));
    }
    stale = with(stale, "isSidechain", json!(true));
    let line = parsed(&stale);
    assert!(!line.final_line);
    assert!(line.sidechain);
    assert_eq!(line.speed, None);
    assert_eq!(line.counts, counts(10, 5, 200, 100, 2000, 0));
    let absent = without(final_line(), "message/stop_reason");
    assert!(!parsed(&absent).final_line, "an absent stop_reason is null");
}

#[test]
fn zero_top_level_falls_back_to_the_first_iteration() {
    let mut zero = final_line();
    for field in [
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "cache_creation/ephemeral_5m_input_tokens",
        "cache_creation/ephemeral_1h_input_tokens",
    ] {
        zero = with(zero, &format!("message/usage/{field}"), json!(0));
    }
    zero = with(
        zero,
        "message/usage/iterations",
        json!([{"input_tokens": 3, "output_tokens": 50, "cache_read_input_tokens": 500,
                "cache_creation_input_tokens": 40,
                "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 40}},
               {"input_tokens": 999, "output_tokens": 999}]),
    );
    assert_eq!(
        parsed(&zero).counts,
        counts(3, 50, 0, 40, 500, 2),
        "the first iteration's counts; web searches stay the top level's"
    );
    let differs = with(
        final_line(),
        "message/usage/iterations",
        json!([{"input_tokens": 3, "output_tokens": 50}]),
    );
    assert_eq!(
        parsed(&differs).counts,
        counts(10, 120, 200, 100, 2000, 2),
        "a non-zero top level wins"
    );
    let empty = with(zero, "message/usage/iterations", json!([]));
    assert_eq!(parsed(&empty).counts, counts(0, 0, 0, 0, 0, 2));
}

#[test]
fn a_huge_iterations_array_keeps_only_the_first() {
    // Spec §2.2: only iterations[0] counts; the rest is skipped as it is read.
    let mut zero = final_line();
    for field in [
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "cache_creation/ephemeral_5m_input_tokens",
        "cache_creation/ephemeral_1h_input_tokens",
    ] {
        zero = with(zero, &format!("message/usage/{field}"), json!(0));
    }
    let mut many = vec![json!({"input_tokens": 7, "output_tokens": 8})];
    many.extend(std::iter::repeat_n(json!({}), 200_000));
    let line = with(zero, "message/usage/iterations", Value::Array(many));
    assert_eq!(parsed(&line).counts, counts(7, 8, 0, 0, 0, 2));
    let null = with(final_line(), "message/usage/iterations", Value::Null);
    assert_eq!(parsed(&null).counts, counts(10, 120, 200, 100, 2000, 2));
}

#[test]
fn cache_writes_without_a_split_count_as_five_minute_writes() {
    let unsplit = without(final_line(), "message/usage/cache_creation");
    assert_eq!(parsed(&unsplit).counts, counts(10, 120, 300, 0, 2000, 2));
    let short = with(
        final_line(),
        "message/usage/cache_creation",
        json!({"ephemeral_1h_input_tokens": 100}),
    );
    assert_eq!(
        parsed(&short).counts,
        counts(10, 120, 200, 100, 2000, 2),
        "what the split leaves out is 5-minute"
    );
}

#[test]
fn a_missing_request_id_keys_on_an_empty_one() {
    // Ruling R5.
    let line = parsed(&without(final_line(), "requestId"));
    assert_eq!(
        (line.message_id.as_str(), line.request_id.as_str()),
        ("msg_1", "")
    );
}

#[test]
fn synthetic_lines_are_skipped() {
    let mut synthetic = with(final_line(), "message/model", json!("<synthetic>"));
    synthetic = without(synthetic, "requestId");
    synthetic = with(
        synthetic,
        "message/usage",
        json!({"input_tokens": 0, "output_tokens": 0}),
    );
    assert_eq!(parse(&synthetic), Ok(None));
    let bare = without(synthetic, "message/usage");
    assert_eq!(parse(&bare), Ok(None), "skipped before its usage is read");
}

#[test]
fn other_line_types_are_none() {
    let user = json!({"parentUuid": null, "isSidechain": false, "cwd": "/w/app",
                      "sessionId": "s1", "type": "user",
                      "message": {"role": "user", "content": "x"},
                      "uuid": "u1", "timestamp": "2026-10-14T09:00:00.000Z"});
    let nested = json!({"type": "user", "toolUseResult": {"type": "assistant"},
                        "message": {"role": "user", "content": "x"}});
    let untyped = json!({"toolUseResult": {"type": "assistant"}});
    for line in [
        user,
        nested,
        untyped,
        json!({"type": "system", "subtype": "informational", "content": "x"}),
        json!({"type": "summary", "summary": "x", "leafUuid": "u1"}),
        with(final_line(), "type", json!("progress")),
    ] {
        assert_eq!(parse(&line), Ok(None), "{line}");
    }
    for bytes in [
        &b""[..],
        b"\n",
        b"not json at all",
        b"\xff\xfe\x00",
        br#"{"type":"user"#,
    ] {
        assert_eq!(parse_usage_line(bytes), Ok(None));
    }
}

#[test]
fn missing_usage_is_an_error() {
    for gone in [
        "message/usage",
        "message/id",
        "message/model",
        "message",
        "sessionId",
        "timestamp",
        "message/usage/input_tokens",
        "message/usage/output_tokens",
    ] {
        assert!(
            parse(&without(final_line(), gone)).is_err(),
            "without {gone}"
        );
    }
    for timestamp in ["2026-10-14", "yesterday", "2026-10-14T09:00:02.123"] {
        let line = with(final_line(), "timestamp", json!(timestamp));
        assert!(parse(&line).is_err(), "{timestamp}");
    }
}

#[test]
fn the_space_after_the_colon_is_tolerated() {
    let spaced = serde_json::to_string(&final_line())
        .unwrap()
        .replace("\":", "\": ");
    assert!(spaced.contains(r#""type": "assistant""#));
    assert_eq!(
        parse_usage_line(spaced.as_bytes()).unwrap().unwrap().counts,
        counts(10, 120, 200, 100, 2000, 2)
    );
}

#[test]
fn errors_never_quote_the_line() {
    let compact = serde_json::to_string(&final_line()).unwrap();
    let lines = [
        with(
            final_line(),
            "message/usage/input_tokens",
            json!("SENTINEL"),
        ),
        with(final_line(), "message/usage/speed", json!(["SENTINEL"])),
        with(final_line(), "timestamp", json!("SENTINEL")),
        with(final_line(), "isSidechain", json!("SENTINEL")),
    ]
    .into_iter()
    .map(|v| serde_json::to_string(&v).unwrap())
    .chain([format!("{}SENTINEL", &compact[..compact.len() - 1])]);
    for line in lines {
        let e = parse_usage_line(line.as_bytes()).expect_err(&line);
        assert!(!e.to_string().contains("SENTINEL"), "{e}");
    }
}

/// The count fields a hostile value may land in, as the compact line spells them first (the
/// top level comes before `iterations`).
const COUNT_FIELDS: [&str; 7] = [
    r#""input_tokens":10"#,
    r#""output_tokens":120"#,
    r#""cache_creation_input_tokens":300"#,
    r#""cache_read_input_tokens":2000"#,
    r#""ephemeral_5m_input_tokens":200"#,
    r#""ephemeral_1h_input_tokens":100"#,
    r#""web_search_requests":2"#,
];

const HOSTILE: [&str; 8] = [
    "-1",
    "1.5",
    "1e3",
    r#""7""#,
    "null",
    "true",
    "[]",
    "18446744073709551616",
];

proptest! {
    #[test]
    fn hostile_lines_are_errors_never_panics(
        bytes in proptest::collection::vec(any::<u8>(), 0..512),
        field in 0..COUNT_FIELDS.len(),
        value in 0..HOSTILE.len(),
        cut in 0usize..2_000,
    ) {
        // Review Focus 2: arbitrary bytes, alone and behind an assistant prefix, never panic.
        let _ = parse_usage_line(&bytes);
        let mut prefixed = br#"{"type":"assistant","message":"#.to_vec();
        prefixed.extend_from_slice(&bytes);
        let _ = parse_usage_line(&prefixed);
        // A count that is negative, fractional, text, null or above u64 is an error.
        let good = serde_json::to_string(&final_line()).unwrap();
        let (key, _) = COUNT_FIELDS[field].split_once(':').unwrap();
        let bad = good.replacen(COUNT_FIELDS[field], &format!("{key}:{}", HOSTILE[value]), 1);
        prop_assert_ne!(&bad, &good);
        prop_assert!(parse_usage_line(bad.as_bytes()).is_err(), "{}", bad);
        // A line cut short is never a usage line.
        let cut = cut % good.len();
        prop_assert!(!matches!(parse_usage_line(&good.as_bytes()[..cut]), Ok(Some(_))));
    }
}

/// Builds `rel` under `root` as a file holding `{}`.
fn touch(root: &Path, rel: &str) -> PathBuf {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, "{}\n").unwrap();
    p
}

#[test]
fn discovers_sessions_and_subagents_only() {
    // Spec §2.1: two fixed levels; nothing deeper and no other extension.
    let d = tempfile::tempdir().unwrap();
    let projects = d.path().join("projects");
    let mut expected = vec![
        touch(&projects, "-w-a/s1.jsonl"),
        touch(&projects, "-w-a/s2.jsonl"),
        touch(&projects, "-w-a/s1/subagents/agent-0123456789abcdef.jsonl"),
        touch(&projects, "-w-b/s3.jsonl"),
    ];
    touch(&projects, "-w-a/notes.txt");
    touch(
        &projects,
        "-w-a/s1/subagents/agent-0123456789abcdef.meta.json",
    );
    touch(&projects, "-w-a/s1/subagents/other.jsonl");
    touch(&projects, "-w-a/s1/subagents/deeper/agent-1.jsonl");
    touch(&projects, "-w-a/s1/tool-results/agent-2.jsonl");
    touch(&projects, "-w-a/s1/s1.jsonl");
    touch(&projects, "top.jsonl");
    fs::create_dir_all(projects.join("-w-b/dir.jsonl")).unwrap();
    // A link to a file is followed; a dangling one and a link to a directory are not.
    let elsewhere = touch(d.path(), "elsewhere/s4.jsonl");
    symlink(&elsewhere, projects.join("-w-b/s4.jsonl")).unwrap();
    expected.push(projects.join("-w-b/s4.jsonl"));
    symlink(
        d.path().join("nowhere.jsonl"),
        projects.join("-w-b/gone.jsonl"),
    )
    .unwrap();
    touch(d.path(), "linked-project/s5.jsonl");
    symlink(d.path().join("linked-project"), projects.join("-w-c")).unwrap();
    touch(d.path(), "linked-agents/agent-fedcba9876543210.jsonl");
    fs::create_dir_all(projects.join("-w-b/s3")).unwrap();
    symlink(
        d.path().join("linked-agents"),
        projects.join("-w-b/s3/subagents"),
    )
    .unwrap();
    expected.sort();
    assert_eq!(discover(&projects).unwrap(), expected);
}

#[test]
fn a_missing_projects_dir_is_empty() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        discover(&d.path().join("projects")).unwrap(),
        Vec::<PathBuf>::new()
    );
    let file = touch(d.path(), "projects-file");
    assert_eq!(
        discover(&file).unwrap(),
        Vec::<PathBuf>::new(),
        "not a directory"
    );
    fs::create_dir(d.path().join("empty")).unwrap();
    assert_eq!(
        discover(&d.path().join("empty")).unwrap(),
        Vec::<PathBuf>::new()
    );
}

/// The fixture tree under `tests/fixtures/transcripts/projects`.
fn fixture_projects() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/transcripts/projects")
}

/// One response as the merge rule of spec §2.2 sees it.
#[derive(Debug, PartialEq)]
struct Merged {
    session_id: String,
    model: String,
    sidechain: bool,
    first_ms: i64,
    counts: TokenCounts,
    finalized: bool,
    lines: usize,
}

#[test]
fn the_fixture_tree_parses_to_known_responses() {
    // The numbers the engine's refresh tests can rely on: four responses over eight usage
    // lines in four files, one response copied into a resumed session, two unreadable lines.
    let files = discover(&fixture_projects()).unwrap();
    let names: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(fixture_projects())
                .unwrap()
                .display()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        [
            "-w-app/11111111-1111-4111-8111-111111111111/subagents/agent-0123456789abcdef.jsonl",
            "-w-app/11111111-1111-4111-8111-111111111111.jsonl",
            "-w-app/22222222-2222-4222-8222-222222222222.jsonl",
            "-w-lib/33333333-3333-4333-8333-333333333333.jsonl",
        ]
    );
    let (mut usage_lines, mut errors) = (0, 0);
    let mut merged: BTreeMap<(String, String), Merged> = BTreeMap::new();
    for file in &files {
        for line in fs::read(file).unwrap().split(|b| *b == b'\n') {
            let u = match parse_usage_line(line) {
                Ok(Some(u)) => u,
                Ok(None) => continue,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            };
            usage_lines += 1;
            let m = merged
                .entry((u.message_id.clone(), u.request_id.clone()))
                .or_insert_with(|| Merged {
                    session_id: u.session_id.clone(),
                    model: u.model.clone(),
                    sidechain: u.sidechain,
                    first_ms: u.at_ms,
                    counts: TokenCounts::default(),
                    finalized: false,
                    lines: 0,
                });
            m.first_ms = m.first_ms.min(u.at_ms);
            m.counts.merge_max(&u.counts);
            m.finalized |= u.final_line;
            m.lines += 1;
        }
    }
    assert_eq!((usage_lines, errors), (8, 2));
    let s1 = "11111111-1111-4111-8111-111111111111";
    let s2 = "22222222-2222-4222-8222-222222222222";
    let r = |id: &str, req: &str| merged.get(&(id.to_owned(), req.to_owned())).unwrap();
    assert_eq!(merged.len(), 4);
    assert_eq!(
        r("msg_A", "req_A"),
        &Merged {
            session_id: s1.into(),
            model: "claude-opus-5-5".into(),
            sidechain: false,
            first_ms: 1_791_968_401_100,
            counts: counts(10, 120, 200, 100, 2000, 1),
            finalized: true,
            lines: 4,
        }
    );
    assert_eq!(
        r("msg_B", "req_B"),
        &Merged {
            session_id: s1.into(),
            model: "claude-sonnet-5-5".into(),
            sidechain: false,
            first_ms: 1_791_968_700_000,
            counts: counts(3, 50, 40, 0, 500, 0),
            finalized: true,
            lines: 1,
        }
    );
    assert_eq!(
        r("msg_C", "req_C"),
        &Merged {
            session_id: s1.into(),
            model: "claude-haiku-4-5-20251001".into(),
            sidechain: true,
            first_ms: 1_791_968_460_000,
            counts: counts(7, 8, 64, 0, 1000, 0),
            finalized: false,
            lines: 2,
        }
    );
    assert_eq!(
        r("msg_D", "req_D"),
        &Merged {
            session_id: s2.into(),
            model: "claude-opus-5-5".into(),
            sidechain: false,
            first_ms: 1_791_972_000_000,
            counts: counts(20, 30, 0, 0, 0, 0),
            finalized: true,
            lines: 1,
        }
    );
}
