//! The token report's compatibility checks (token report §15, §17 T1): a real request's
//! transcript carries every field the token index reads, and a session record carries the
//! `sessionId` its sightings key on, with what `/clear` does to it. Evidence is field names,
//! counts and model names only, never a transcript's content.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use super::{new_record, read_json, records};
use crate::compat::ctx::{Ctx, MODEL, PROMPT};
use crate::compat::report::{Outcome, Probe};
use crate::compat::sys::{HarnessError, pause, wait_until};

const REQUEST: [&str; 6] = ["-p", PROMPT, "--model", MODEL, "--max-turns", "1"];

/// What every assistant line carries that the index needs to read it (token report §3.2,
/// §5.4), as JSON pointers.
const REQUIRED: [&str; 9] = [
    "/requestId",
    "/sessionId",
    "/timestamp",
    "/message/id",
    "/message/model",
    "/message/usage/input_tokens",
    "/message/usage/output_tokens",
    "/message/usage/cache_creation_input_tokens",
    "/message/usage/cache_read_input_tokens",
];

/// What a response's final line, the one with a `stop_reason`, carries besides (token report
/// §2.1).
const ON_FINAL: [&str; 4] = [
    "/cwd",
    "/isSidechain",
    "/message/usage/cache_creation/ephemeral_5m_input_tokens",
    "/message/usage/cache_creation/ephemeral_1h_input_tokens",
];

/// What only a final line may carry (token report §2.2): recorded, never required.
const OPTIONAL: [&str; 3] = [
    "/message/usage/speed",
    "/message/usage/iterations",
    "/message/usage/server_tool_use/web_search_requests",
];

/// Whether `v` has the field at `pointer` as the index reads it: a token count is an unsigned
/// integer, `isSidechain` a boolean, anything else a string. A null `stop_reason` is absent.
fn has(v: &Value, pointer: &str) -> bool {
    match v.pointer(pointer) {
        Some(n) if pointer.ends_with("_tokens") => n.as_u64().is_some(),
        Some(b) if pointer == "/isSidechain" => b.is_boolean(),
        Some(s) => s.is_string(),
        None => false,
    }
}

/// What a transcript's assistant lines carry, by field name and count.
#[derive(Debug, Default, PartialEq, Eq)]
struct Audit {
    /// Assistant lines, `<synthetic>` ones left out (token report §2.2).
    lines: usize,
    /// Of `REQUIRED`, how many lines lack each field that some line lacks.
    missing: BTreeMap<&'static str, usize>,
    /// Lines with a `stop_reason`.
    finals: usize,
    /// Of `ON_FINAL`, how many final lines lack each field that some final line lacks.
    missing_on_final: BTreeMap<&'static str, usize>,
    /// Of `OPTIONAL`, how many final lines carry each.
    optional_on_final: BTreeMap<&'static str, usize>,
    models: BTreeSet<String>,
    /// Lines that are not JSON.
    unparsed: usize,
}

impl Audit {
    fn add(&mut self, text: &str) {
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                self.unparsed += 1;
                continue;
            };
            let model = v.pointer("/message/model").and_then(Value::as_str);
            if v["type"] != "assistant" || model == Some("<synthetic>") {
                continue;
            }
            self.lines += 1;
            if let Some(model) = model {
                self.models.insert(model.to_owned());
            }
            for field in REQUIRED.into_iter().filter(|f| !has(&v, f)) {
                *self.missing.entry(field).or_default() += 1;
            }
            if !has(&v, "/message/stop_reason") {
                continue;
            }
            self.finals += 1;
            for field in ON_FINAL.into_iter().filter(|f| !has(&v, f)) {
                *self.missing_on_final.entry(field).or_default() += 1;
            }
            for field in OPTIONAL {
                let seen = self.optional_on_final.entry(field).or_default();
                if v.pointer(field).is_some() {
                    *seen += 1;
                }
            }
        }
    }

    /// What was appended to `path` past its first `from` bytes.
    fn add_appended(&mut self, path: &Path, from: u64) -> io::Result<()> {
        let bytes = fs::read(path)?;
        let from = usize::try_from(from).map_or(bytes.len(), |n| n.min(bytes.len()));
        self.add(&String::from_utf8_lossy(&bytes[from..]));
        Ok(())
    }
}

/// The session transcripts under `projects`, `<project>/<sessionId>.jsonl` (token report
/// §2.1), with their lengths. Subagent transcripts, a level deeper, are left out.
fn transcript_files(projects: &Path) -> BTreeMap<PathBuf, u64> {
    let mut out = BTreeMap::new();
    let Ok(dirs) = fs::read_dir(projects) else {
        return out;
    };
    for dir in dirs.flatten() {
        let Ok(files) = fs::read_dir(dir.path()) else {
            continue;
        };
        for path in files.flatten().map(|f| f.path()) {
            let Ok(m) = fs::metadata(&path) else {
                continue;
            };
            if m.is_file() && path.extension().is_some_and(|x| x == "jsonl") {
                out.insert(path, m.len());
            }
        }
    }
    out
}

/// The transcripts that are new in `after` or longer than in `before`, each with its length
/// before (0 for a new one).
fn grown(before: &BTreeMap<PathBuf, u64>, after: &BTreeMap<PathBuf, u64>) -> Vec<(PathBuf, u64)> {
    after
        .iter()
        .filter(|(path, len)| before.get(*path).is_none_or(|was| *len > was))
        .map(|(path, _)| (path.clone(), before.get(path).copied().unwrap_or(0)))
        .collect()
}

/// The sessions, by their transcripts' names, that a final assistant line was written to
/// between `before` and `after`: where an answer went.
fn answered_in(before: &BTreeMap<PathBuf, u64>, after: &BTreeMap<PathBuf, u64>) -> Vec<String> {
    grown(before, after)
        .into_iter()
        .filter(|(path, from)| {
            let mut a = Audit::default();
            a.add_appended(path, *from).is_ok() && a.finals > 0
        })
        .filter_map(|(path, _)| Some(path.file_stem()?.to_str()?.to_owned()))
        .collect()
}

/// The `sessionId` of the session record at `path`, read from its JSON; `None` while it is
/// partly written, or gone.
fn record_session_id(path: &Path) -> Option<String> {
    read_json(path)?["sessionId"].as_str().map(str::to_owned)
}

/// The `sessionId`s of the record files in `current` that are neither `record` nor in `before`:
/// a record CC replaced or added.
fn new_record_ids(current: &[PathBuf], before: &[PathBuf], record: &Path) -> Vec<String> {
    let mut ids: Vec<String> = current
        .iter()
        .filter(|path| path.as_path() != record && !before.contains(path))
        .filter_map(|path| record_session_id(path))
        .collect();
    ids.sort();
    ids
}

/// The `sessionId` the session's record names once the next answer is written. `read` is the
/// record's own id, `last` the last one read while it lived (kept when it was caught
/// mid-write). A record file that is gone is named by the one record that replaced it.
fn named_session_id(
    read: Option<String>,
    record_kept: bool,
    last: Option<String>,
    new_ids: &[String],
) -> Option<String> {
    match (read, record_kept, new_ids) {
        (Some(id), _, _) => Some(id),
        (None, true, _) => last,
        (None, false, [only]) => Some(only.clone()),
        (None, false, _) => None,
    }
}

/// The summary of the `/clear` check: the final answer on the record's `sessionId`, from the
/// id at the start and the one it names after the next answer.
fn clear_summary(first: Option<&str>, named: Option<&str>) -> &'static str {
    match (first, named) {
        (None, _) => "the record carried no sessionId at the start; the notes say what /clear did",
        (Some(_), None) => {
            "after /clear the record names no session; the notes say what became of it"
        }
        (Some(a), Some(b)) if a == b => {
            "/clear left the record's sessionId as it was by the next answer"
        }
        (Some(_), Some(_)) => "/clear changed the record's sessionId by the next answer",
    }
}

/// Token report §15: a real request's transcript has every field the index reads on every
/// assistant line, and its final line has the rest.
pub fn transcript_usage_fields(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    Ok(Probe::run(|p| {
        let (_, spelling) = ctx.profile_ready()?;
        let projects = Path::new(&spelling).join("projects");
        let before = transcript_files(&projects);
        let ran = ctx.run_profile(&REQUEST)?;
        p.expect("claude -p succeeded", ran.success(), ran.summary());
        let written = grown(&before, &transcript_files(&projects));
        p.expect(
            "the request was written to a session transcript under projects/",
            !written.is_empty(),
            json!(written.len()),
        );
        let mut a = Audit::default();
        for (path, from) in &written {
            a.add_appended(path, *from)?;
        }
        p.expect("an assistant line was written", a.lines > 0, json!(a.lines));
        p.expect(
            "every assistant line has the ids, time, model and counts the index reads",
            a.missing.is_empty(),
            json!(a.missing),
        );
        p.expect(
            "a final line, with a stop_reason, was written",
            a.finals > 0,
            json!(a.finals),
        );
        p.expect(
            "every final line has cwd, isSidechain and the 5m/1h cache-write split",
            a.missing_on_final.is_empty(),
            json!(a.missing_on_final),
        );
        p.note("the models", json!(a.models));
        p.note(
            "optional fields on final lines (information only)",
            json!(a.optional_on_final),
        );
        p.note("lines that are not JSON", json!(a.unparsed));
        Ok("a real request's transcript carries every field the token index reads")
    }))
}

/// Token report §17 T1: a session record carries a `sessionId` that names the session's
/// transcript, and what `/clear` does to it: whether the record's `sessionId` changes, and
/// whether the conversation after it is written to the transcript the record then names. The
/// check passes whatever `/clear` does; the notes are the finding.
pub fn session_record_session_id_and_clear(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    Ok(Probe::run(|p| {
        let (_, spelling) = ctx.profile_ready()?;
        let home = Path::new(&spelling);
        let sessions = home.join("sessions");
        let projects = home.join("projects");
        let before = records(&sessions);
        let known: Vec<PathBuf> = before.iter().map(|(path, _)| path.clone()).collect();
        let mut pty = ctx.profile_pty(&["--model", MODEL])?;
        let Some((record, _)) = new_record(&sessions, &before, Duration::from_secs(60)) else {
            p.expect(
                "a session record at the start",
                false,
                json!(pty.transcript(600)),
            );
            pty.finish(Duration::from_secs(5))?;
            return Ok("no session record was written");
        };
        let first = record_session_id(&record);
        p.expect(
            "the record carries a string sessionId",
            first.is_some(),
            json!(null),
        );

        // A conversation to clear: one prompt, answered in the transcript the record names.
        pause(Duration::from_secs(2));
        let files = transcript_files(&projects);
        pty.line(PROMPT)?;
        let mut answered = Vec::new();
        wait_until(Duration::from_secs(90), || {
            answered = answered_in(&files, &transcript_files(&projects));
            !answered.is_empty()
        });
        p.expect(
            "the prompt was answered",
            !answered.is_empty(),
            json!(answered.len()),
        );
        p.expect(
            "the answer is in the transcript the record's sessionId names",
            first.as_ref().is_some_and(|id| answered.contains(id)),
            json!(null),
        );

        // `/clear`, then the record, watched for 10 s. A record caught mid-write keeps the
        // last id read.
        pty.line("/clear")?;
        let mut now = first.clone();
        let changed = wait_until(Duration::from_secs(10), || {
            now = record_session_id(&record).or_else(|| now.clone());
            now != first
        });
        let current: Vec<PathBuf> = records(&sessions)
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        let new_ids = new_record_ids(&current, &known, &record);
        p.note(
            "after /clear: what happened to the record (token report §17 T1)",
            json!({
                "sessionIdChangedWithin10s": changed,
                "recordFileKept": record.exists(),
                "newRecordFiles": current.iter().filter(|c| **c != record && !known.contains(c)).count(),
                "newRecordSessionIds": new_ids,
            }),
        );

        // The next prompt: which transcript the answer goes to, against what the record says.
        pause(Duration::from_secs(2));
        let files = transcript_files(&projects);
        pty.line(PROMPT)?;
        let mut answered = Vec::new();
        wait_until(Duration::from_secs(90), || {
            answered = answered_in(&files, &transcript_files(&projects));
            !answered.is_empty()
        });
        p.expect(
            "after /clear: the next prompt was answered",
            !answered.is_empty(),
            json!(answered.len()),
        );
        let current: Vec<PathBuf> = records(&sessions)
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        let new_ids = new_record_ids(&current, &known, &record);
        let named = named_session_id(record_session_id(&record), record.exists(), now, &new_ids);
        p.note(
            "after /clear: where the next answer was written (token report §17 T1)",
            json!({
                "sessionIdChangedByNextAnswer": named != first,
                "aNewTranscript": answered.iter().any(|s| Some(s) != first.as_ref()),
                "theTranscriptTheRecordNames": answered.iter().any(|s| Some(s) == named.as_ref()),
                "recordFileKept": record.exists(),
                "newRecordSessionIds": new_ids,
                "aNewRecordNamesTheTranscript": answered.iter().any(|s| new_ids.contains(s)),
            }),
        );
        let _ = pty.line("/exit");
        p.note(
            "the session's exit",
            json!(pty.finish(Duration::from_secs(30))?),
        );
        Ok(clear_summary(first.as_deref(), named.as_deref()))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::keychain::random_hex;

    fn audit(text: &str) -> Audit {
        let mut a = Audit::default();
        a.add(text);
        a
    }

    /// A response as CC writes it, with no real content: a user line, the response's stale
    /// snapshot and its final line, a `<synthetic>` line, and a line that is not JSON.
    fn transcript() -> String {
        let usage = json!({
            "input_tokens": 3,
            "output_tokens": 8,
            "cache_creation_input_tokens": 1200,
            "cache_read_input_tokens": 40000,
            "cache_creation": {"ephemeral_5m_input_tokens": 1200, "ephemeral_1h_input_tokens": 0},
        });
        let line = |stop: Value, usage: &Value| {
            json!({
                "type": "assistant",
                "requestId": "req_1",
                "sessionId": "s1",
                "timestamp": "2026-10-14T10:00:00.000Z",
                "cwd": "/w",
                "isSidechain": false,
                "message": {
                    "id": "msg_1",
                    "model": "claude-haiku-4-5-20251001",
                    "stop_reason": stop,
                    "usage": usage,
                    "content": [{"type": "text", "text": "x"}],
                },
            })
            .to_string()
        };
        let mut last = usage.clone();
        last["output_tokens"] = json!(333);
        last["speed"] = json!("standard");
        [
            json!({"type": "user", "sessionId": "s1", "message": {"content": "x"}}).to_string(),
            line(Value::Null, &usage),
            line(json!("end_turn"), &last),
            json!({"type": "assistant", "message": {"id": "m2", "model": "<synthetic>"}})
                .to_string(),
            "{not json".to_owned(),
        ]
        .join("\n")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-{name}-{}", random_hex().unwrap()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_audit_counts_lines_finals_and_models() {
        let a = audit(&transcript());
        assert_eq!(a.lines, 2, "the synthetic line is left out");
        assert!(
            a.missing.is_empty() && a.missing_on_final.is_empty(),
            "{a:?}"
        );
        assert_eq!(a.finals, 1, "a null stop_reason is no final line");
        assert_eq!(
            a.models,
            BTreeSet::from(["claude-haiku-4-5-20251001".to_owned()])
        );
        assert_eq!(a.unparsed, 1);
        assert_eq!(
            a.optional_on_final,
            BTreeMap::from([
                ("/message/usage/iterations", 0),
                ("/message/usage/server_tool_use/web_search_requests", 0),
                ("/message/usage/speed", 1),
            ])
        );
    }

    #[test]
    fn a_field_missing_or_of_another_type_is_named_with_its_count() {
        let final_line = transcript().lines().nth(2).unwrap().to_owned();
        let mut v: Value = serde_json::from_str(&final_line).unwrap();
        v.as_object_mut().unwrap().remove("requestId");
        v["message"]["usage"]["output_tokens"] = json!(-1);
        v["isSidechain"] = json!("no");
        v["message"]["usage"]["cache_creation"] = Value::Null;
        let a = audit(&format!("{v}\n{v}\n"));
        assert_eq!(
            a.missing,
            BTreeMap::from([("/message/usage/output_tokens", 2), ("/requestId", 2)])
        );
        assert_eq!(
            a.missing_on_final,
            BTreeMap::from([
                ("/isSidechain", 2),
                ("/message/usage/cache_creation/ephemeral_1h_input_tokens", 2),
                ("/message/usage/cache_creation/ephemeral_5m_input_tokens", 2),
            ])
        );
    }

    #[test]
    fn only_session_transcripts_are_listed_and_growth_is_found() {
        let root = scratch("transcripts");
        let project = root.join("-w");
        fs::create_dir_all(project.join("s1/subagents")).unwrap();
        fs::write(project.join("s1.jsonl"), "a\n").unwrap();
        fs::write(
            project.join("s1/subagents/agent-0123456789abcdef.jsonl"),
            "b\n",
        )
        .unwrap();
        fs::write(project.join("notes.txt"), "c\n").unwrap();
        let before = transcript_files(&root);
        assert_eq!(before, BTreeMap::from([(project.join("s1.jsonl"), 2)]));
        assert!(transcript_files(&root.join("missing")).is_empty());
        fs::write(project.join("s1.jsonl"), "a\nb\n").unwrap();
        fs::write(project.join("s2.jsonl"), "").unwrap();
        let after = transcript_files(&root);
        assert_eq!(
            grown(&before, &after),
            [(project.join("s1.jsonl"), 2), (project.join("s2.jsonl"), 0)]
        );
        assert!(grown(&after, &after).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_answer_is_found_only_past_where_its_transcript_was() {
        let root = scratch("answers");
        let project = root.join("-w");
        fs::create_dir_all(&project).unwrap();
        let text = transcript();
        fs::write(project.join("s1.jsonl"), &text).unwrap();
        let before = transcript_files(&root);
        assert!(answered_in(&before, &before).is_empty());
        // s1 gains a user line only; s2 is new and holds a final line.
        fs::write(
            project.join("s1.jsonl"),
            format!("{text}\n{{\"type\":\"user\"}}\n"),
        )
        .unwrap();
        fs::write(project.join("s2.jsonl"), &text).unwrap();
        assert_eq!(answered_in(&before, &transcript_files(&root)), ["s2"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_record_s_session_id_is_read_from_its_json() {
        let dir = scratch("records");
        fs::write(dir.join("1.json"), r#"{"pid":1,"sessionId":"abc"}"#).unwrap();
        fs::write(dir.join("2.json"), r#"{"pid":2}"#).unwrap();
        fs::write(dir.join("3.json"), r#"{"pid":3,"sessi"#).unwrap();
        assert_eq!(
            record_session_id(&dir.join("1.json")).as_deref(),
            Some("abc")
        );
        assert_eq!(record_session_id(&dir.join("2.json")), None);
        assert_eq!(
            record_session_id(&dir.join("3.json")),
            None,
            "partly written"
        );
        assert_eq!(record_session_id(&dir.join("4.json")), None, "gone");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_replaced_record_is_found_by_the_new_files_session_ids() {
        let dir = scratch("replaced");
        let old = dir.join("1.json");
        let kept = dir.join("2.json");
        let new = dir.join("3.json");
        fs::write(&kept, r#"{"pid":2,"sessionId":"other"}"#).unwrap();
        fs::write(&new, r#"{"pid":3,"sessionId":"fresh"}"#).unwrap();
        fs::write(dir.join("4.json"), r#"{"pid":4,"sessi"#).unwrap();
        let before = [old.clone(), kept.clone()];
        let current = [kept, new, dir.join("4.json")];
        assert_eq!(new_record_ids(&current, &before, &old), ["fresh"]);
        assert!(new_record_ids(&before, &before, &old).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_named_id_follows_the_record_its_replacement_or_the_last_read() {
        let id = |s: &str| Some(s.to_owned());
        let fresh = ["fresh".to_owned()];
        let two = ["a".to_owned(), "b".to_owned()];
        assert_eq!(named_session_id(id("new"), true, id("old"), &[]), id("new"));
        assert_eq!(
            named_session_id(None, true, id("old"), &[]),
            id("old"),
            "mid-write"
        );
        assert_eq!(
            named_session_id(None, false, id("old"), &fresh),
            id("fresh")
        );
        assert_eq!(named_session_id(None, false, id("old"), &two), None);
        assert_eq!(named_session_id(None, false, id("old"), &[]), None);
    }

    #[test]
    fn the_summary_states_the_final_answer_not_the_first_sample() {
        assert_eq!(
            clear_summary(Some("a"), Some("b")),
            "/clear changed the record's sessionId by the next answer"
        );
        assert_eq!(
            clear_summary(Some("a"), Some("a")),
            "/clear left the record's sessionId as it was by the next answer"
        );
        assert!(clear_summary(Some("a"), None).contains("names no session"));
        assert!(clear_summary(None, None).contains("no sessionId at the start"));
    }
}
