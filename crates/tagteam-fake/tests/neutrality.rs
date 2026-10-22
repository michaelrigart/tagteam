//! Keeps the provider-neutrality boundary: no neutral crate names Claude Code, and only the
//! binary's composition root names `tagteam-cc`. The scan reads production source text.

use std::fs;
use std::path::{Path, PathBuf};

const NEUTRAL: [&str; 5] = [
    "tagteam-provider",
    "tagteam-core",
    "tagteam-engine",
    "tagteam-daemon",
    "tagteam-tui",
];
const LEAKS: [&str; 3] = ["claude", "anthropic", "tagteam_cc"];
/// Claude Code's field names and record kinds, matched in their exact case: the neutral
/// `ProcStart` type is not the field `procStart`.
const FIELDS: [&str; 5] = [
    "procStart",
    "oauthAccount",
    "accountUuid",
    "claudeAiOauth",
    "daemon-worker",
];
/// Held for the TUI redesign, pinned by file and text. Each must still match, so an entry goes
/// once its line does.
const ALLOWED: [(&str, &str); 0] = [];
const COMPOSITION_ROOT: &str = "crates/tagteam/src/app/context.rs";
const BINARY_ALLOWED: [(&str, &str); 2] = [
    ("crates/tagteam/src/logging.rs", "tagteam_cc=info"),
    ("crates/tagteam/src/logging.rs", "tagteam_cc=debug"),
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn rust_files(dir: &Path, into: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, into);
        } else if path.extension().is_some_and(|e| e == "rs") {
            into.push(path);
        }
    }
}

/// Every `.rs` under `src`, sorted, less the files a parent declares as `#[cfg(test)] mod name;`.
fn sources(src: &Path) -> Vec<PathBuf> {
    let mut all = Vec::new();
    rust_files(src, &mut all);
    all.sort();
    let mut skipped_files = Vec::new();
    let mut skipped_dirs = Vec::new();
    for file in &all {
        let text = fs::read_to_string(file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let stem = file.file_stem().unwrap().to_string_lossy();
        let base = if matches!(&*stem, "lib" | "main" | "mod") {
            file.parent().unwrap().to_path_buf()
        } else {
            file.with_extension("")
        };
        for at in 0..lines.len() {
            if let Some((name, false)) = test_mod(&lines, at) {
                skipped_files.push(base.join(format!("{name}.rs")));
                skipped_dirs.push(base.join(name));
            }
        }
    }
    all.retain(|f| !skipped_files.contains(f) && !skipped_dirs.iter().any(|d| f.starts_with(d)));
    all
}

/// `#[cfg(test)]` on a line of its own, then the next line that is not blank and not another
/// attribute: `[pub[(…)] ]mod name;` (a test-only file) or `… mod name {` (inline).
fn test_mod(lines: &[&str], at: usize) -> Option<(String, bool)> {
    if lines[at].trim() != "#[cfg(test)]" {
        return None;
    }
    let next = lines[at + 1..]
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.starts_with("#["))?;
    let rest = ["pub(crate) ", "pub(super) ", "pub "]
        .iter()
        .find_map(|v| next.strip_prefix(v))
        .unwrap_or(next);
    let decl = rest.strip_prefix("mod ")?;
    let name: String = decl
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    Some((
        name.clone(),
        decl[name.len()..].trim_start().starts_with('{'),
    ))
}

/// The production half: everything before the first inline `#[cfg(test)] mod`, without lines
/// whose trimmed text starts with `//`.
fn production(text: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let end = (0..lines.len())
        .find(|&i| matches!(test_mod(&lines, i), Some((_, true))))
        .unwrap_or(lines.len());
    (0..end)
        .filter(|&i| !lines[i].trim_start().starts_with("//"))
        .map(|i| (i + 1, lines[i].to_owned()))
        .collect()
}

/// Whether a production line of a neutral crate names Claude Code: a `LEAKS` word in any case,
/// or a `FIELDS` name exactly.
fn names_claude_code(line: &str) -> bool {
    let lower = line.to_lowercase();
    LEAKS.iter().any(|w| lower.contains(w)) || FIELDS.iter().any(|f| line.contains(f))
}

#[test]
fn the_scanner_keeps_production_and_drops_test_halves_and_comments() {
    assert_eq!(
        production("fn a() {}\n// claude\n#[cfg(test)]\nmod tests {\n    claude\n}\n"),
        [(1, "fn a() {}".to_owned())]
    );
    assert_eq!(
        production("fn a() {}\n#[cfg(test)]\n#[allow(dead_code)]\npub(crate) mod t {\nclaude\n}\n"),
        [(1, "fn a() {}".to_owned())]
    );
    assert_eq!(
        production("fn a() {}\n// procStart\n#[cfg(test)]\nmod tests {\n    \"oauthAccount\"\n}\n"),
        [(1, "fn a() {}".to_owned())]
    );
    assert_eq!(
        production("#[cfg(test)]\nmod testutil;\nfn b() {}\n").last(),
        Some(&(3, "fn b() {}".to_owned()))
    );
    assert_eq!(
        test_mod(&["#[cfg(test)]", "pub(crate) mod testutil;"], 0),
        Some(("testutil".to_owned(), false))
    );
}

#[test]
fn a_field_name_leaks_only_in_its_exact_case() {
    for line in [
        "o.get(\"procStart\")",
        "\"daemon-worker\"",
        "v[\"oauthAccount\"]",
        "accountUuid",
        "claudeAiOauth",
        "Claude",
    ] {
        assert!(names_claude_code(line), "{line}");
    }
    for line in [
        "start: ProcStart,",
        "proc_start",
        "account_uuid",
        "daemon_worker",
    ] {
        assert!(!names_claude_code(line), "{line}");
    }
}

#[test]
fn no_neutral_crate_names_claude_code() {
    let root = workspace();
    let mut hits = Vec::new();
    let mut matched = [false; ALLOWED.len()];
    let mut engine_files = 0;
    for krate in NEUTRAL {
        for file in sources(&root.join("crates").join(krate).join("src")) {
            if krate == "tagteam-engine" {
                engine_files += 1;
            }
            let name = relative(&root, &file);
            let text = fs::read_to_string(&file).unwrap();
            for (line, body) in production(&text) {
                if !names_claude_code(&body) {
                    continue;
                }
                let allowed = ALLOWED
                    .iter()
                    .position(|(f, t)| *f == name && body.contains(t));
                match allowed {
                    Some(i) => matched[i] = true,
                    None => hits.push(format!("{name}:{line}: {}", body.trim())),
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "a neutral crate names Claude Code:\n{}",
        hits.join("\n")
    );
    for (i, (file, text)) in ALLOWED.iter().enumerate() {
        assert!(
            matched[i],
            "{file} no longer contains {text:?}: remove the allowlist entry once its line is gone"
        );
    }
    assert!(
        engine_files >= 30,
        "the scan saw only {engine_files} tagteam-engine files"
    );
}

/// The lines of a manifest's non-dev dependency sections (any `dependencies` header, with or
/// without a target, and a trailing comment) that name `tagteam-cc`, whether renamed or as a
/// `[dependencies.tagteam-cc]` table.
fn cc_dependencies(manifest: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let mut in_scope = false;
    for line in manifest.lines() {
        let text = line.trim();
        if text.starts_with('[') {
            let header = text.split('#').next().unwrap().trim();
            in_scope = header.contains("dependencies") && !header.contains("dev-dependencies");
            if in_scope && header.ends_with(".tagteam-cc]") {
                hits.push(text.to_owned());
            }
        } else if in_scope && !text.starts_with('#') && text.contains("tagteam-cc") {
            hits.push(text.to_owned());
        }
    }
    hits
}

#[test]
fn no_neutral_crate_depends_on_tagteam_cc() {
    let root = workspace();
    let mut hits = Vec::new();
    for krate in NEUTRAL {
        let manifest =
            fs::read_to_string(root.join("crates").join(krate).join("Cargo.toml")).unwrap();
        hits.extend(
            cc_dependencies(&manifest)
                .into_iter()
                .map(|l| format!("{krate}: {l}")),
        );
    }
    assert!(
        hits.is_empty(),
        "a neutral crate depends on tagteam-cc:\n{}",
        hits.join("\n")
    );
}

#[test]
fn only_the_composition_root_names_tagteam_cc_in_the_binary() {
    let root = workspace();
    let mut hits = Vec::new();
    for file in sources(&root.join("crates/tagteam/src")) {
        let name = relative(&root, &file);
        if name == COMPOSITION_ROOT {
            continue;
        }
        let text = fs::read_to_string(&file).unwrap();
        for (line, body) in production(&text) {
            let lower = body.to_lowercase();
            if !LEAKS.iter().any(|w| lower.contains(w)) {
                continue;
            }
            if BINARY_ALLOWED
                .iter()
                .any(|(f, t)| *f == name && body.contains(t))
            {
                continue;
            }
            hits.push(format!("{name}:{line}: {}", body.trim()));
        }
    }
    assert!(
        hits.is_empty(),
        "only {COMPOSITION_ROOT} may name tagteam-cc or Claude Code:\n{}",
        hits.join("\n")
    );
    let root_text = fs::read_to_string(root.join(COMPOSITION_ROOT)).unwrap();
    assert!(
        root_text.contains("ClaudeCode"),
        "{COMPOSITION_ROOT} no longer names ClaudeCode: the composition root has moved"
    );
}

#[test]
fn the_dependency_check_sees_every_form_and_skips_dev_dependencies() {
    let manifest = "\
[package]
name = \"x\"

[dependencies]  # runtime
tagteam-core.workspace = true
cc = { package = \"tagteam-cc\", workspace = true }

[target.'cfg(unix)'.dependencies]
tagteam-cc.workspace = true

[build-dependencies.tagteam-cc]
workspace = true

[dependencies.tagteam-cc]
workspace = true

[dev-dependencies]
tagteam-cc.workspace = true

[dev-dependencies.tagteam-cc]
workspace = true
";
    assert_eq!(
        cc_dependencies(manifest),
        [
            "cc = { package = \"tagteam-cc\", workspace = true }",
            "tagteam-cc.workspace = true",
            "[build-dependencies.tagteam-cc]",
            "[dependencies.tagteam-cc]",
        ]
    );
}
