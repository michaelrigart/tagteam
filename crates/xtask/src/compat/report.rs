//! The compat report (§15.4 "Result"): one entry per check with its evidence, as JSON and as
//! Markdown under `target/compat/`, and the exit code: 0 when every check passed, 1 when one
//! failed, 2 when the harness itself failed. Evidence never holds a token, key, password or
//! email: a secret appears only as `fingerprint`'s short hash. What a child printed can still
//! carry one, so it goes through `Redactor` as it is captured (`Cmd::redact`), before anything
//! formats it, and every string a report writes goes through it again (`Report::write`).

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const EXIT_PASS: u8 = 0;
pub const EXIT_FAIL: u8 = 1;
pub const EXIT_HARNESS: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    /// Not applicable here (a macOS-only check on Linux, a missing optional account).
    Skip,
    /// The harness failed inside the check; the check proves nothing.
    Error,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "fail",
            Status::Skip => "skip",
            Status::Error => "error",
        }
    }
}

/// The first 12 hex digits of `secret`'s SHA-256: enough to tell two values apart in a report,
/// never enough to use one.
pub fn fingerprint(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))[..12].to_owned()
}

/// What a report must never carry, and what stands in for it: each identity value of the
/// compat store's accounts (`learn`), and any token-shaped run (`<token>`): one holding
/// `sk-ant-`, or 40 or more characters of `[A-Za-z0-9_-]`. Where readability and privacy
/// conflict, privacy wins.
#[derive(Clone, PartialEq, Default)]
pub struct Redactor {
    /// Longest value first, so a value inside another never splits it.
    known: Vec<(String, String)>,
}

/// What a `{:?}` shows: how many values it learned, never the values.
impl fmt::Debug for Redactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Redactor")
            .field("learned", &self.known.len())
            .finish_non_exhaustive()
    }
}

impl Redactor {
    /// `value` is written as `placeholder` wherever it appears, whatever its length, and so is
    /// its JSON-escaped form, which a child's JSON output carries. One under four characters is
    /// replaced only as a whole word, where no letter or digit touches it, so that an
    /// organization named `AI` goes and the word `MAIL` stays.
    pub fn learn(&mut self, value: &str, placeholder: String) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        let quoted = Value::from(value).to_string();
        let escaped = &quoted[1..quoted.len() - 1];
        for form in [value, escaped] {
            if !self.known.iter().any(|(v, _)| v == form) {
                self.known.push((form.to_owned(), placeholder.clone()));
            }
        }
        self.known.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    }

    /// `text` for a child's raw output.
    pub fn bytes(&self, raw: &[u8]) -> Vec<u8> {
        self.text(&String::from_utf8_lossy(raw)).into_bytes()
    }

    /// A child's output as it may be shown. JSON is redacted by structure (`value`), so its
    /// keywords, numbers and shape are never touched; any other output as text (`bytes`).
    pub fn output(&self, raw: &[u8]) -> Vec<u8> {
        let trimmed = raw.trim_ascii();
        match serde_json::from_slice::<Value>(trimmed) {
            Ok(v) if !trimmed.is_empty() => {
                let mut out = serde_json::to_vec(&self.value(&v)).expect("a Value serializes");
                out.push(b'\n');
                out
            }
            _ => self.bytes(raw),
        }
    }

    /// `s` as it may be shown. Token-shaped runs are found on the ORIGINAL text and replaced
    /// first; learned values are replaced only in what lies between them, so an identity
    /// placeholder can never split a token and leave the tail of its secret below the pattern.
    pub fn text(&self, s: &str) -> String {
        let mut redacted = String::with_capacity(s.len());
        let mut plain = String::new();
        let mut run = String::new();
        let flush = |run: &mut String, plain: &mut String, redacted: &mut String| {
            if run.len() >= 40 || run.contains("sk-ant-") {
                redacted.push_str(&self.identities(plain));
                plain.clear();
                redacted.push_str("<token>");
            } else {
                plain.push_str(run);
            }
            run.clear();
        };
        for c in s.chars() {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                run.push(c);
            } else {
                flush(&mut run, &mut plain, &mut redacted);
                plain.push(c);
            }
        }
        flush(&mut run, &mut plain, &mut redacted);
        redacted.push_str(&self.identities(&plain));
        redacted
    }

    /// Every learned value in `s` as its placeholder (a short one only as a whole word).
    fn identities(&self, s: &str) -> String {
        let mut out = s.to_owned();
        for (value, placeholder) in &self.known {
            out = if value.chars().count() < 4 {
                replace_word(&out, value, placeholder)
            } else {
                out.replace(value.as_str(), placeholder)
            };
        }
        out
    }

    /// `v` by its structure: every string value as `text`, and an object key only where it is
    /// exactly a learned value. Keywords, numbers and the shape stay as they are, so an
    /// organization named `accounts` or `null` breaks nothing.
    pub fn value(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(a) => a.iter().map(|x| self.value(x)).collect(),
            Value::Object(o) => o
                .iter()
                .map(|(k, x)| (self.key(k), self.value(x)))
                .collect::<serde_json::Map<_, _>>()
                .into(),
            other => other.clone(),
        }
    }

    fn key(&self, k: &str) -> String {
        self.known
            .iter()
            .find(|(value, _)| value == k)
            .map_or_else(|| k.to_owned(), |(_, placeholder)| placeholder.clone())
    }

    /// `outcome` as it may be stored: its summary and every evidence entry redacted. A check's
    /// `expect_eq` compares raw values and records both, so this is where the raw view of what
    /// it saw is kept out of the report (`run_checks`).
    pub fn outcome(&self, outcome: &Outcome) -> Outcome {
        Outcome {
            status: outcome.status,
            summary: self.text(&outcome.summary),
            evidence: self.evidence(&outcome.evidence),
        }
    }

    fn evidence(&self, evidence: &[Evidence]) -> Vec<Evidence> {
        evidence
            .iter()
            .map(|e| Evidence {
                label: self.text(&e.label),
                value: self.value(&e.value),
                ok: e.ok,
            })
            .collect()
    }
}

/// `text` with each occurrence of `word` that no letter or digit touches replaced by `with`.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(k) = rest.find(word) {
        let before = rest[..k]
            .chars()
            .next_back()
            .or_else(|| out.chars().next_back());
        let after = rest[k + word.len()..].chars().next();
        let touched = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        out.push_str(&rest[..k]);
        out.push_str(if touched(before) || touched(after) {
            word
        } else {
            with
        });
        rest = &rest[k + word.len()..];
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    pub label: String,
    pub value: Value,
    /// `Some` for an expectation, `None` for a note.
    pub ok: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub status: Status,
    pub summary: String,
    pub evidence: Vec<Evidence>,
}

impl Outcome {
    pub fn skip(reason: impl Into<String>) -> Self {
        Self {
            status: Status::Skip,
            summary: reason.into(),
            evidence: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>, evidence: Vec<Evidence>) -> Self {
        Self {
            status: Status::Error,
            summary: message.into(),
            evidence,
        }
    }
}

/// A check's evidence as it is gathered: notes, and expectations that fail the check.
#[derive(Debug, Default)]
pub struct Probe {
    evidence: Vec<Evidence>,
    failed: Vec<String>,
}

impl Probe {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn note(&mut self, label: &str, value: impl Into<Value>) {
        self.evidence.push(Evidence {
            label: label.to_owned(),
            value: value.into(),
            ok: None,
        });
    }

    /// Records `value` under `label`; the check fails unless `ok`. Returns `ok`.
    pub fn expect(&mut self, label: &str, ok: bool, value: impl Into<Value>) -> bool {
        self.evidence.push(Evidence {
            label: label.to_owned(),
            value: value.into(),
            ok: Some(ok),
        });
        if !ok {
            self.failed.push(label.to_owned());
        }
        ok
    }

    pub fn expect_eq(
        &mut self,
        label: &str,
        expected: impl Into<Value>,
        actual: impl Into<Value>,
    ) -> bool {
        let (expected, actual) = (expected.into(), actual.into());
        let ok = expected == actual;
        self.expect(label, ok, json!({"expected": expected, "actual": actual}))
    }

    /// Pass with `passed` as the summary, or fail naming every unmet expectation.
    pub fn finish(self, passed: impl Into<String>) -> Outcome {
        let (status, summary) = if self.failed.is_empty() {
            (Status::Pass, passed.into())
        } else {
            (
                Status::Fail,
                format!("not as expected: {}", self.failed.join(", ")),
            )
        };
        Outcome {
            status,
            summary,
            evidence: self.evidence,
        }
    }

    pub fn into_evidence(self) -> Vec<Evidence> {
        self.evidence
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    pub id: &'static str,
    pub title: &'static str,
    pub outcome: Outcome,
    pub seconds: f64,
}

#[derive(Clone, PartialEq, Default)]
pub struct Report {
    /// Epoch seconds.
    pub started_at: i64,
    pub platform: &'static str,
    /// What `claude --version` reported, when it ran.
    pub claude: Option<String>,
    pub tested: Option<String>,
    /// What the harness found before the checks (paths, managed settings, the account).
    pub setup: Vec<Evidence>,
    /// Why the harness stopped, when it did: the run then exits 2.
    pub harness_error: Option<String>,
    pub checks: Vec<CheckResult>,
    /// The version `--bless` wrote.
    pub blessed: Option<String>,
    /// The signal that ended the run, which then exits 128 + its number.
    pub interrupted: Option<i32>,
    /// The teardown steps that completed, once teardown has begun (`None` before it).
    pub teardown: Option<Vec<&'static str>>,
    /// What `write` keeps out of the files.
    pub redact: Redactor,
}

/// A `{:?}` shows the report as `write` would: through its redactor, which it never shows.
impl fmt::Debug for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let r = self.redacted();
        f.debug_struct("Report")
            .field("started_at", &r.started_at)
            .field("platform", &r.platform)
            .field("claude", &r.claude)
            .field("tested", &r.tested)
            .field("setup", &r.setup)
            .field("harness_error", &r.harness_error)
            .field("checks", &r.checks)
            .field("blessed", &r.blessed)
            .field("interrupted", &r.interrupted)
            .field("teardown", &r.teardown)
            .finish_non_exhaustive()
    }
}

fn evidence_json(e: &[Evidence]) -> Value {
    e.iter()
        .map(|e| json!({"label": e.label, "ok": e.ok, "value": e.value}))
        .collect()
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

impl Report {
    pub fn exit_code(&self) -> u8 {
        let has = |s: Status| self.checks.iter().any(|c| c.outcome.status == s);
        if let Some(n) = self.interrupted {
            128 + n as u8
        } else if self.harness_error.is_some() || has(Status::Error) {
            EXIT_HARNESS
        } else if has(Status::Fail) {
            EXIT_FAIL
        } else {
            EXIT_PASS
        }
    }

    /// Whether each of `ids` ran and passed, and nothing failed: what `--bless` requires.
    pub fn passed_all(&self, ids: &[&str]) -> bool {
        self.exit_code() == EXIT_PASS
            && ids.iter().all(|id| {
                self.checks
                    .iter()
                    .any(|c| c.id == *id && c.outcome.status == Status::Pass)
            })
    }

    fn count(&self, s: Status) -> usize {
        self.checks.iter().filter(|c| c.outcome.status == s).count()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "schemaVersion": 1,
            "startedAt": tagteam_cc::usage::format_iso8601(self.started_at),
            "platform": self.platform,
            "claudeVersion": self.claude,
            "testedVersion": self.tested,
            "exitCode": self.exit_code(),
            "harnessError": self.harness_error,
            "blessed": self.blessed,
            "setup": evidence_json(&self.setup),
            "checks": self.checks.iter().map(|c| json!({
                "id": c.id,
                "title": c.title,
                "status": c.outcome.status.as_str(),
                "summary": c.outcome.summary,
                "seconds": (c.seconds * 10.0).round() / 10.0,
                "evidence": evidence_json(&c.outcome.evidence),
            })).collect::<Vec<_>>(),
        })
    }

    pub fn to_markdown(&self) -> String {
        let mut md = String::from("# tagteam compat report\n\n");
        md += &format!(
            "- Started: {} ({})\n",
            tagteam_cc::usage::format_iso8601(self.started_at),
            self.platform
        );
        md += &format!(
            "- claude: {} (tested: {})\n",
            self.claude.as_deref().unwrap_or("not run"),
            self.tested.as_deref().unwrap_or("unknown")
        );
        md += &format!(
            "- Result: {} passed, {} failed, {} skipped, {} errors; exit {}\n",
            self.count(Status::Pass),
            self.count(Status::Fail),
            self.count(Status::Skip),
            self.count(Status::Error),
            self.exit_code()
        );
        if let Some(v) = &self.blessed {
            md += &format!("- Blessed: `compat/tested-cc-version` is now {v}\n");
        }
        if let Some(e) = &self.harness_error {
            md += &format!("- **Harness failure:** {e}\n");
        }
        if !self.setup.is_empty() {
            md += "\n## Setup\n\n";
            md += &evidence_md(&self.setup);
        }
        if !self.checks.is_empty() {
            md += "\n| Check | Status | Summary |\n|---|---|---|\n";
            for c in &self.checks {
                md += &format!(
                    "| `{}` | {} | {} |\n",
                    c.id,
                    c.outcome.status.as_str(),
                    cell(&c.outcome.summary)
                );
            }
        }
        for c in &self.checks {
            md += &format!(
                "\n## `{}`: {}\n\n{} in {:.1} s: {}\n\n",
                c.id,
                c.title,
                c.outcome.status.as_str(),
                c.seconds,
                c.outcome.summary
            );
            md += &evidence_md(&c.outcome.evidence);
        }
        md
    }

    /// The report as it may be written: every string a child could have put in it (setup and
    /// check evidence, summaries, the harness error, `claude`'s version) through `redact`.
    pub fn redacted(&self) -> Report {
        let r = &self.redact;
        let text = |s: &Option<String>| s.as_deref().map(|s| r.text(s));
        Report {
            claude: text(&self.claude),
            tested: text(&self.tested),
            setup: r.evidence(&self.setup),
            harness_error: text(&self.harness_error),
            checks: self
                .checks
                .iter()
                .map(|c| CheckResult {
                    outcome: r.outcome(&c.outcome),
                    ..c.clone()
                })
                .collect(),
            ..self.clone()
        }
    }

    /// `report.json` and `report.md` in `dir`, created if needed, from `redacted`: the one
    /// place a report is written, and the backstop behind the redaction at capture.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        let report = self.redacted();
        fs::create_dir_all(dir)?;
        let mut json = serde_json::to_vec_pretty(&report.to_json()).map_err(io::Error::other)?;
        json.push(b'\n');
        fs::write(dir.join("report.json"), json)?;
        fs::write(dir.join("report.md"), report.to_markdown())
    }
}

fn evidence_md(evidence: &[Evidence]) -> String {
    evidence
        .iter()
        .map(|e| {
            let mark = match e.ok {
                Some(true) => "✓ ",
                Some(false) => "✗ ",
                None => "",
            };
            format!("- {mark}{}: `{}`\n", e.label, e.value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_learned_value_inside_a_token_cannot_split_it_and_leave_its_tail() {
        // Codex slice 10 re-review: identity replacement ran first, so `Acme` inside the token
        // became a placeholder, and the last 12 secret characters fell below the token pattern.
        let mut r = Redactor::default();
        r.learn("Acme", "<account 1 org>".into());
        let token = format!("sk-ant-oat01-{}Acme{}", "A".repeat(44), "B".repeat(12));
        assert_eq!(r.text(&token), "<token>");
        let around = format!("org Acme used {token} for Acme");
        assert_eq!(
            r.text(&around),
            "org <account 1 org> used <token> for <account 1 org>"
        );
        assert!(!r.text(&token).contains('B'));
        // Structure and bytes go the same way.
        assert_eq!(
            r.value(&json!({"k": token.clone()})),
            json!({"k": "<token>"})
        );
        assert_eq!(r.bytes(token.as_bytes()), b"<token>");
    }

    fn result(id: &'static str, status: Status) -> CheckResult {
        CheckResult {
            id,
            title: "a check",
            outcome: Outcome {
                status,
                summary: format!("{id} | done"),
                evidence: vec![Evidence {
                    label: "seen".into(),
                    value: json!({"n": 1}),
                    ok: Some(status != Status::Fail),
                }],
            },
            seconds: 1.25,
        }
    }

    fn report(statuses: &[(&'static str, Status)]) -> Report {
        Report {
            started_at: 1_790_000_000,
            platform: "macos",
            claude: Some("2.1.290".into()),
            tested: Some("2.1.286".into()),
            checks: statuses.iter().map(|(id, s)| result(id, *s)).collect(),
            ..Report::default()
        }
    }

    #[test]
    fn the_exit_code_is_0_for_a_pass_1_for_a_failure_and_2_for_the_harness() {
        assert_eq!(
            report(&[("a", Status::Pass), ("b", Status::Skip)]).exit_code(),
            0
        );
        assert_eq!(
            report(&[("a", Status::Pass), ("b", Status::Fail)]).exit_code(),
            1
        );
        assert_eq!(
            report(&[("a", Status::Fail), ("b", Status::Error)]).exit_code(),
            2
        );
        let mut stopped = report(&[("a", Status::Pass)]);
        stopped.harness_error = Some("claude not found".into());
        assert_eq!(stopped.exit_code(), 2);
    }

    #[test]
    fn a_probe_fails_on_any_unmet_expectation_and_names_it() {
        let mut p = Probe::new();
        p.note("spelling", "/tmp/x");
        assert!(p.expect_eq("loggedIn", true, true));
        assert!(!p.expect_eq("authMethod", "claude.ai", "none"));
        let o = p.finish("every outcome as expected");
        assert_eq!(o.status, Status::Fail);
        assert_eq!(o.summary, "not as expected: authMethod");
        assert_eq!(
            o.evidence[2].value,
            json!({"expected": "claude.ai", "actual": "none"})
        );
        let mut p = Probe::new();
        p.expect("present", true, true);
        assert_eq!(p.finish("fine").status, Status::Pass);
    }

    #[test]
    fn bless_needs_every_named_check_passed_and_no_failure() {
        let r = report(&[("a", Status::Pass), ("b", Status::Pass)]);
        assert!(r.passed_all(&["a", "b"]));
        assert!(!r.passed_all(&["a", "b", "c"]), "c did not run");
        let r = report(&[("a", Status::Pass), ("b", Status::Skip)]);
        assert!(!r.passed_all(&["a", "b"]), "a skip is not a pass");
    }

    #[test]
    fn json_and_markdown_carry_each_check_with_its_evidence() {
        let r = report(&[("auth-status", Status::Pass), ("config-lock", Status::Fail)]);
        let j = r.to_json();
        assert_eq!(j["schemaVersion"], 1);
        assert_eq!(j["exitCode"], 1);
        assert_eq!(j["startedAt"], "2026-09-21T14:13:20Z");
        assert_eq!(j["checks"][1]["id"], "config-lock");
        assert_eq!(j["checks"][1]["status"], "fail");
        assert_eq!(j["checks"][1]["seconds"], 1.3);
        assert_eq!(j["checks"][1]["evidence"][0]["ok"], false);
        let md = r.to_markdown();
        assert!(md.contains("- claude: 2.1.290 (tested: 2.1.286)\n"), "{md}");
        assert!(
            md.contains("| `auth-status` | pass | auth-status \\| done |\n"),
            "{md}"
        );
        assert!(
            md.contains("## `config-lock`: a check\n\nfail in 1.2 s"),
            "{md}"
        );
        assert!(md.contains("- ✗ seen: `{\"n\":1}`\n"), "{md}");
    }

    #[test]
    fn the_redactor_replaces_what_it_learned_and_any_token_shaped_run() {
        let mut r = Redactor::default();
        r.learn("t@x.co", "<account 1>".into());
        r.learn("t@x.co's Organization", "<account 1 org>".into());
        r.learn("ab", "<short>".into());
        r.learn(" ", "<blank>".into());
        assert_eq!(r.text("t@x.co's Organization"), "<account 1 org>");
        assert_eq!(
            r.text("login t@x.co (ab) ab tab abc ab"),
            "login <account 1> (<short>) <short> tab abc <short>"
        );
        assert_eq!(
            r.text("key=sk-ant-api03-AA, rt aB3_aB3_aB3_aB3_aB3_aB3_aB3_aB3_aB3_aB3_."),
            "key=<token>, rt <token>."
        );
        let kept = "/tmp/tagteam-compat.0123456789ab/homes/one ba7816bf8f01 2.1.290";
        assert_eq!(r.text(kept), kept);
        assert_eq!(
            r.value(&json!({"t@x.co": ["t@x.co", 1, null]})),
            json!({"<account 1>": ["<account 1>", 1, null]})
        );
    }

    #[test]
    fn a_child_s_identity_and_tokens_never_reach_the_written_report() {
        let email = "compat-test@example.com";
        let token = "sk-ant-ort01-Zm9vYmFyYmF6cXV4MTIzNDU2Nzg5MGFiY2RlZmdoaWprbG1u";
        // What a child printed, as `Ran::summary` and `Pty::transcript` carry it.
        let stderr = format!("refreshing {email}: rejected {token}\n");
        let summary = json!({"command": "tagteam list --json", "exit": 1, "stderr": stderr});
        let mut r = report(&[("auth-status", Status::Fail)]);
        r.redact.learn(email, "<account 1>".into());
        r.setup.push(Evidence {
            label: "a transcript".into(),
            value: json!(format!("> /login\nLogged in as {email}\n")),
            ok: None,
        });
        r.harness_error = Some(format!("tagteam list failed: {summary}"));
        r.checks[0].outcome.summary = format!("{email} {token}");
        r.checks[0].outcome.evidence.push(Evidence {
            label: format!("claude auth status for {email}"),
            value: summary,
            ok: Some(false),
        });
        let dir = std::env::temp_dir().join(format!("xtask-report-{}", std::process::id()));
        r.write(&dir).unwrap();
        for name in ["report.json", "report.md"] {
            let written = fs::read_to_string(dir.join(name)).unwrap();
            assert!(!written.contains(email), "{name}: {written}");
            assert!(!written.contains("sk-ant-"), "{name}: {written}");
            assert!(written.contains("<account 1>"), "{name}: {written}");
            assert!(written.contains("<token>"), "{name}: {written}");
        }
        fs::remove_dir_all(&dir).unwrap();
        assert!(
            r.to_json().to_string().contains(email),
            "only the files are redacted"
        );
    }

    #[test]
    fn a_short_identity_is_redacted_as_a_word_from_both_report_files() {
        let mut r = report(&[("auth-status", Status::Fail)]);
        r.redact.learn("AI", "<account 1 org>".into());
        r.checks[0].outcome.evidence.push(Evidence {
            label: "claude auth status".into(),
            value: json!({"stderr": "MAIL for AI (AI's seat): orgName=AI\n"}),
            ok: Some(false),
        });
        let dir = std::env::temp_dir().join(format!("xtask-report-ai-{}", std::process::id()));
        r.write(&dir).unwrap();
        for name in ["report.json", "report.md"] {
            let written = fs::read_to_string(dir.join(name)).unwrap();
            assert!(
                written
                    .split(|c: char| !c.is_alphanumeric())
                    .all(|w| w != "AI"),
                "{name}: {written}"
            );
            assert!(
                written.contains("MAIL for <account 1 org> (<account 1 org>'s seat)"),
                "{name}: {written}"
            );
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_fingerprint_is_short_and_stable() {
        assert_eq!(fingerprint("abc"), "ba7816bf8f01");
        assert_ne!(fingerprint("rt-a"), fingerprint("rt-b"));
    }

    #[test]
    fn an_outcome_is_redacted_as_it_is_stored() {
        // `expect_eq` records the raw actual value: here CC reported the account's email.
        let mut r = Redactor::default();
        r.learn("t@x.co", "<account 1>".into());
        let mut p = Probe::new();
        p.expect_eq("another identity: email", "other@x.co", "t@x.co");
        let raw = p.finish("fine");
        assert!(raw.evidence[0].value.to_string().contains("t@x.co"));
        let stored = r.outcome(&raw);
        assert_eq!(
            stored.evidence[0].value,
            json!({"expected": "other@x.co", "actual": "<account 1>"})
        );
        assert_eq!(stored.status, Status::Fail);
        assert!(!format!("{stored:?}").contains("t@x.co"));
    }

    #[test]
    fn debug_output_shows_no_learned_value() {
        let mut r = report(&[("auth-status", Status::Fail)]);
        r.redact.learn("t@x.co", "<account 1>".into());
        r.harness_error = Some("refresh for t@x.co failed".into());
        for shown in [format!("{r:?}"), format!("{:#?}", r.redact)] {
            assert!(!shown.contains("t@x.co"), "{shown}");
        }
        assert!(format!("{r:?}").contains("<account 1>"));
    }

    #[test]
    fn a_value_with_a_quote_is_redacted_from_text_that_holds_only_its_escaped_form() {
        // A truncated JSON line is no JSON, so it is redacted as text; the organization's name
        // appears in it only as JSON escapes it (`\"`), which the raw form does not match.
        let org = r#"Acme "Research""#;
        let mut r = Redactor::default();
        r.learn(org, "<account 1 org>".into());
        let line = r#"{"organizationName":"Acme \"Research\"","seats":4"#;
        assert!(!line.contains(org), "only the escaped form is in the text");
        let shown = r.text(line);
        assert!(!shown.contains("Research"), "{shown}");
        assert_eq!(shown, r#"{"organizationName":"<account 1 org>","seats":4"#);
        assert_eq!(r.text(org), "<account 1 org>");
    }
}
