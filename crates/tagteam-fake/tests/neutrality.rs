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
const LEAKS: [&str; 2] = ["claude", "anthropic"];
/// Held for the TUI redesign, pinned by file and text. Each must still match, so an entry goes
/// once its line does.
const ALLOWED: [(&str, &str); 0] = [];
const COMPOSITION_ROOT: &str = "crates/tagteam/src/app.rs";
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
        production("#[cfg(test)]\nmod testutil;\nfn b() {}\n").last(),
        Some(&(3, "fn b() {}".to_owned()))
    );
    assert_eq!(
        test_mod(&["#[cfg(test)]", "pub(crate) mod testutil;"], 0),
        Some(("testutil".to_owned(), false))
    );
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
                let lower = body.to_lowercase();
                if !LEAKS.iter().any(|w| lower.contains(w)) {
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

#[test]
fn no_neutral_crate_depends_on_tagteam_cc() {
    let root = workspace();
    let mut hits = Vec::new();
    for krate in NEUTRAL {
        let manifest =
            fs::read_to_string(root.join("crates").join(krate).join("Cargo.toml")).unwrap();
        let mut in_dependencies = false;
        for line in manifest.lines() {
            if line.trim() == "[dependencies]" {
                in_dependencies = true;
            } else if line.starts_with('[') {
                in_dependencies = false;
            } else if in_dependencies && line.trim().starts_with("tagteam-cc") {
                hits.push(format!("{krate}: {}", line.trim()));
            }
        }
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
            if !(body.contains("tagteam_cc") || body.contains("ClaudeCode")) {
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
        "only {COMPOSITION_ROOT} may name tagteam-cc:\n{}",
        hits.join("\n")
    );
    let root_text = fs::read_to_string(root.join(COMPOSITION_ROOT)).unwrap();
    assert!(
        root_text.contains("ClaudeCode"),
        "{COMPOSITION_ROOT} no longer names ClaudeCode: the composition root has moved"
    );
}
