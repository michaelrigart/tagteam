//! FakeAgent's transcripts (token report spec §3.2): the engine's token tests write them through
//! `FakeUsage`, `usage_line_json` and `append_transcript`.

use std::fs;
use std::path::PathBuf;

use tagteam_core::tokens::TokenCounts;
use tagteam_fake::{FakeAgent, FakePaths, FakeUsage, append_transcript, usage_line_json};
use tagteam_provider::{Env, Provider};

fn fx() -> (tempfile::TempDir, Env) {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    (d, env)
}

#[test]
fn round_trips_a_usage_line() {
    let fake = FakeAgent::new();
    let stale = FakeUsage {
        message_id: "m2".into(),
        request_id: String::new(),
        session_id: "s2".into(),
        at_ms: -5,
        model: "fake-2".into(),
        project: None,
        sidechain: true,
        counts: TokenCounts {
            input: u64::MAX,
            web_search: 9,
            ..TokenCounts::default()
        },
        speed: Some("fast".into()),
        final_line: false,
    };
    for u in [FakeUsage::default(), stale] {
        let line = usage_line_json(&u);
        assert!(!line.contains('\n'));
        assert_eq!(
            fake.parse_usage_line(line.as_bytes()),
            Ok(Some(u.usage_line()))
        );
    }
    let without_req = br#"{"t":"usage","id":"m","session":"s","at":1,"model":"fake-1","side":false,"final":true,"in":1,"out":1,"cw5":0,"cw1":0,"cr":0,"ws":0}"#;
    let line = fake.parse_usage_line(without_req).unwrap().unwrap();
    assert_eq!(
        (line.request_id.as_str(), line.project, line.speed),
        ("", None, None),
        "ruling R5: no req keys on an empty one"
    );
}

#[test]
fn other_types_are_none() {
    let fake = FakeAgent::new();
    for line in [&br#"{"t":"note","id":"m1"}"#[..], br#"{}"#, br#"{"t":7}"#] {
        assert_eq!(fake.parse_usage_line(line), Ok(None));
    }
    let good = usage_line_json(&FakeUsage::default());
    for bad in [
        "not json".to_owned(),
        "[1]".to_owned(),
        r#"{"t":"usage"}"#.to_owned(),
        good.replace(r#""in":1"#, r#""in":-1"#),
        good.replace(r#""out":2"#, r#""out":"2""#),
        good.replace(r#""side":false"#, r#""side":null"#),
        good.replace(r#""cwd":"/w""#, r#""cwd":7"#),
        good[..good.len() - 1].to_owned(),
    ] {
        assert!(fake.parse_usage_line(bad.as_bytes()).is_err(), "{bad}");
    }
}

#[test]
fn discovers_only_jsonl() {
    let (_d, env) = fx();
    let fake = FakeAgent::new();
    assert_eq!(
        fake.transcript_files(&env).unwrap(),
        Vec::<PathBuf>::new(),
        "no directory yet"
    );
    let a = append_transcript(&env, "a.jsonl", "");
    let b = append_transcript(&env, "b.jsonl", "{}\n");
    append_transcript(&env, "notes.txt", "x\n");
    let dir = FakePaths::resolve(&env).transcripts;
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("sub/c.jsonl"), "{}\n").unwrap();
    fs::create_dir_all(dir.join("d.jsonl")).unwrap();
    assert_eq!(fake.transcript_files(&env).unwrap(), vec![a, b]);
}

#[test]
fn appending_writes_text_as_it_is() {
    let (_d, env) = fx();
    let first = usage_line_json(&FakeUsage::default());
    let path = append_transcript(&env, "s.jsonl", &format!("{first}\n"));
    append_transcript(&env, "s.jsonl", "{\"t\":\"usa");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("{first}\n{{\"t\":\"usa")
    );
    assert_eq!(
        path,
        FakePaths::resolve(&env).dir.join("transcripts/s.jsonl")
    );
}

#[test]
fn its_records_prices_and_capability() {
    let (_d, env) = fx();
    let fake = FakeAgent::new();
    assert!(fake.capabilities().transcripts);
    assert!(
        !FakeAgent::new()
            .without_transcripts()
            .capabilities()
            .transcripts
    );
    assert_eq!(
        fake.default_session_records_dir(&env),
        Some(env.home.join(".fakeagent/procs"))
    );
    let table = fake.price_table();
    assert_eq!(table.as_of, "2026-01-01");
    assert_eq!(
        table.models.iter().map(|(m, _)| *m).collect::<Vec<_>>(),
        ["fake-1"]
    );
    assert_eq!(table.models[0].1.web_search_per_1k, Some(10.0));
}
