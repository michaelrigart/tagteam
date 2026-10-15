//! Claude Code's transcripts (token report spec §2): the files under the default home's
//! `projects/`, and the usage their assistant lines carry.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::error::Category;
use tagteam_core::time::parse_iso8601_ms;
use tagteam_core::tokens::TokenCounts;
use tagteam_provider::{LineError, UsageLine};

/// `message.model` of a line that is no API call (spec §2.2).
const SYNTHETIC: &str = "<synthetic>";

/// Every transcript under `projects` (spec §2.1, §5.2 step 2): `<project>/<session>.jsonl` and
/// `<project>/<session>/subagents/agent-<id>.jsonl`, sorted. Exactly those two levels are
/// walked; a link to a file is followed, a link to a directory is not. A missing `projects` is
/// empty, and a directory below it that cannot be listed is passed over.
pub fn discover(projects: &Path) -> io::Result<Vec<PathBuf>> {
    let listing = match fs::read_dir(projects) {
        Ok(l) => l,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new());
        }
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for project in listing {
        let project = project?;
        if !is_dir(&project) {
            continue;
        }
        for entry in entries(&project.path()) {
            let path = entry.path();
            if is_dir(&entry) {
                let subagents = path.join("subagents");
                // A link in place of `subagents/` is not followed either.
                if !fs::symlink_metadata(&subagents).is_ok_and(|m| m.is_dir()) {
                    continue;
                }
                for agent in entries(&subagents) {
                    let name = agent.file_name();
                    let name = name.as_encoded_bytes();
                    if name.starts_with(b"agent-") && name.ends_with(b".jsonl") && is_file(&agent) {
                        out.push(agent.path());
                    }
                }
            } else if path.extension().is_some_and(|x| x == "jsonl") && is_file(&entry) {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The entries of `dir`; none when it is missing or cannot be listed.
fn entries(dir: &Path) -> Vec<fs::DirEntry> {
    match fs::read_dir(dir) {
        Ok(listing) => listing.filter_map(Result::ok).collect(),
        Err(e) => {
            if e.kind() != io::ErrorKind::NotFound {
                tracing::debug!(dir = %dir.display(), error = %e, "transcript directory not listed");
            }
            Vec::new()
        }
    }
}

/// A real directory: a link to one is not followed.
fn is_dir(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|t| t.is_dir())
}

/// A regular file, or a link that resolves to one.
fn is_file(entry: &fs::DirEntry) -> bool {
    match entry.file_type() {
        Ok(t) if t.is_symlink() => fs::metadata(entry.path()).is_ok_and(|m| m.is_file()),
        Ok(t) => t.is_file(),
        Err(_) => false,
    }
}

/// The value the prefilter looks for, then checks the key in front of it.
const ASSISTANT: &[u8] = b"\"assistant\"";
/// Horspool's shifts for `ASSISTANT`: how far a window may move when it ends on a byte.
const SHIFT: [u8; 256] = shift_table(ASSISTANT);

const fn shift_table(needle: &[u8]) -> [u8; 256] {
    let last = needle.len() - 1;
    let mut table = [needle.len() as u8; 256];
    let mut i = 0;
    while i < last {
        table[needle[i] as usize] = (last - i) as u8;
        i += 1;
    }
    table
}

/// The first `ASSISTANT` in `line` at or after `at`: a line without one costs about
/// `len / 11` byte reads, not `len`.
fn find_assistant(line: &[u8], mut at: usize) -> Option<usize> {
    let last = ASSISTANT.len() - 1;
    while let Some(&end) = line.get(at + last) {
        if end == b'"' && line[at..].starts_with(ASSISTANT) {
            return Some(at);
        }
        at += usize::from(SHIFT[usize::from(end)]);
    }
    None
}

/// Spec §3.2's prefilter: whether `line` holds `"type":"assistant"`, or `"type": "assistant"`.
/// A line without it is never parsed.
pub(crate) fn may_be_assistant(line: &[u8]) -> bool {
    let mut from = 0;
    while let Some(at) = find_assistant(line, from) {
        let key = &line[..at];
        if key
            .strip_suffix(b" ")
            .unwrap_or(key)
            .ends_with(b"\"type\":")
        {
            return true;
        }
        from = at + 1;
    }
    false
}

/// The fields of a line that spec §3.2 reads; serde skips every other.
#[derive(Deserialize)]
struct Line<'a> {
    #[serde(rename = "type")]
    kind: Option<&'a str>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    timestamp: Option<&'a str>,
    cwd: Option<String>,
    #[serde(rename = "isSidechain", default)]
    sidechain: bool,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    /// Only whether it is null matters (spec §2.2).
    stop_reason: Option<IgnoredAny>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    cache_creation: Option<CacheSplit>,
    server_tool_use: Option<ServerToolUse>,
    speed: Option<String>,
    #[serde(default, deserialize_with = "first_iteration")]
    iterations: Option<Iteration>,
}

/// `usage.iterations[0]`, the rest skipped as it is read and never collected, so a huge array
/// costs no memory (spec §2.2: it is never longer than one; spec §5.2: workers stay bounded).
fn first_iteration<'de, D>(d: D) -> Result<Option<Iteration>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct First;
    impl<'de> serde::de::Visitor<'de> for First {
        type Value = Option<Iteration>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("an array of iterations, or null")
        }

        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let first = seq.next_element::<Iteration>()?;
            while seq.next_element::<IgnoredAny>()?.is_some() {}
            Ok(first)
        }
    }
    d.deserialize_any(First)
}

/// An entry of `usage.iterations` (spec §2.2).
#[derive(Deserialize)]
struct Iteration {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    cache_creation: Option<CacheSplit>,
}

#[derive(Deserialize)]
struct CacheSplit {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

#[derive(Deserialize)]
struct ServerToolUse {
    #[serde(default)]
    web_search_requests: u64,
}

/// The counts of a usage object, web searches aside. The 5-minute and 1-hour writes sum to
/// `written` (spec §2.6); what a line leaves unsplit, as an older line without the split does,
/// counts as 5-minute writes.
fn token_counts(
    input: u64,
    output: u64,
    written: u64,
    read: u64,
    split: Option<&CacheSplit>,
) -> TokenCounts {
    let (m5, h1) = split.map_or((0, 0), |s| {
        (s.ephemeral_5m_input_tokens, s.ephemeral_1h_input_tokens)
    });
    TokenCounts {
        input,
        output,
        cache_write_5m: m5.saturating_add(written.saturating_sub(m5.saturating_add(h1))),
        cache_write_1h: h1,
        cache_read: read,
        web_search: 0,
    }
}

/// A line serde could not read, described without quoting it: serde's own message may.
fn unreadable(e: serde_json::Error) -> LineError {
    let what = match e.classify() {
        Category::Data => "has a field of the wrong type",
        Category::Syntax | Category::Eof | Category::Io => "is not JSON",
    };
    LineError(format!("a transcript line {what} (column {})", e.column()))
}

fn missing(what: &str) -> LineError {
    LineError(format!("an assistant line has no {what}"))
}

/// Spec §3.2: `None` for a line of another type, a `<synthetic>` line, or one the prefilter
/// rules out; `Err` for an assistant line that cannot be read or lacks a field it needs (§5.4).
/// Counts follow spec §2.2: the top level, or `iterations[0]` when the top level is all zero.
pub fn parse_usage_line(line: &[u8]) -> Result<Option<UsageLine>, LineError> {
    if !may_be_assistant(line) {
        return Ok(None);
    }
    let line: Line<'_> = serde_json::from_slice(line).map_err(unreadable)?;
    if line.kind != Some("assistant") {
        return Ok(None);
    }
    let message = line.message.ok_or_else(|| missing("message"))?;
    let model = message.model.ok_or_else(|| missing("model"))?;
    if model == SYNTHETIC {
        return Ok(None);
    }
    let usage = message.usage.ok_or_else(|| missing("usage"))?;
    let message_id = message.id.ok_or_else(|| missing("message id"))?;
    let session_id = line.session_id.ok_or_else(|| missing("session id"))?;
    let at_ms = line
        .timestamp
        .and_then(parse_iso8601_ms)
        .ok_or_else(|| missing("readable timestamp"))?;
    let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) else {
        return Err(missing("input or output count"));
    };
    let top = token_counts(
        input,
        output,
        usage.cache_creation_input_tokens,
        usage.cache_read_input_tokens,
        usage.cache_creation.as_ref(),
    );
    let mut counts = match usage.iterations.as_ref() {
        Some(first) if top == TokenCounts::default() => token_counts(
            first.input_tokens,
            first.output_tokens,
            first.cache_creation_input_tokens,
            first.cache_read_input_tokens,
            first.cache_creation.as_ref(),
        ),
        _ => top,
    };
    counts.web_search = usage.server_tool_use.map_or(0, |s| s.web_search_requests);
    Ok(Some(UsageLine {
        message_id,
        request_id: line.request_id.unwrap_or_default(),
        session_id,
        at_ms,
        model,
        project: line.cwd,
        sidechain: line.sidechain,
        counts,
        speed: usage.speed,
        final_line: message.stop_reason.is_some(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_prefilter_finds_the_type_with_or_without_a_space() {
        for line in [
            &br#"{"type":"assistant"}"#[..],
            br#"{"a":1,"type": "assistant","b":2}"#,
            br#"{"message":{"role":"assistant"},"type":"assistant"}"#,
        ] {
            assert!(may_be_assistant(line), "{}", String::from_utf8_lossy(line));
        }
        for line in [
            &b""[..],
            b"assistant",
            br#"{"type":"user","message":{"role":"assistant"}}"#,
            br#"{"type":"assistants"}"#,
            br#"{"type":  "assistant"}"#,
            br#"{"subtype":"x","role":"assistant"}"#,
            br#"{"type":"assistant"#,
        ] {
            assert!(!may_be_assistant(line), "{}", String::from_utf8_lossy(line));
        }
    }

    /// The prefilter's meaning, spelled the slow way.
    fn naive(line: &[u8]) -> bool {
        [&br#""type":"assistant""#[..], br#""type": "assistant""#]
            .iter()
            .any(|n| line.windows(n.len()).any(|w| w == *n))
    }

    proptest! {
        #[test]
        fn the_prefilter_agrees_with_a_naive_search(
            parts in proptest::collection::vec(
                prop_oneof![
                    Just(br#""type":"assistant""#.to_vec()),
                    Just(br#""type": "assistant""#.to_vec()),
                    Just(br#""assistant""#.to_vec()),
                    Just(br#""type":"#.to_vec()),
                    Just(b" ".to_vec()),
                    proptest::collection::vec(any::<u8>(), 0..24),
                ],
                0..12,
            )
        ) {
            let line = parts.concat();
            prop_assert_eq!(may_be_assistant(&line), naive(&line));
        }
    }
}
