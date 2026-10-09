//! §15.4: the facts `compat/` keeps about Claude Code, which `run`, `doctor`, the weekly drift
//! job and `cargo xtask compat` all read, pinned here against the code that acts on them
//! (Decision 15). The files are read as `scripts/cc-drift.sh` reads them.

use std::collections::BTreeSet;
use std::ffi::OsString;

use tagteam_provider::entry_matches;

use crate::doctor::tested_version;
use crate::session::{CC_MUST_SHARE, CC_PRIVATE, CC_SCRUB, CC_SHARED, session_env};

const KNOWN_SHARED: &str = include_str!("../compat/known-shared");
const KNOWN_PRIVATE: &str = include_str!("../compat/known-private");
const KNOWN_ENV: &str = include_str!("../compat/known-env");
const TESTED_CC_VERSION: &str = include_str!("../compat/tested-cc-version");

/// The two families the drift job extracts from the binary (§15.4).
const FAMILIES: [&str; 2] = ["CLAUDE_CODE_", "ANTHROPIC_"];

/// A file's entries: each line with a trailing CR and its edge whitespace removed, blank lines
/// and `#` comments skipped (the script's `read_list`).
fn entries(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// `known-shared` and `known-private`: bare entry names, each once, none with a path separator.
fn list(text: &str) -> Result<Vec<&str>, String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for name in entries(text) {
        if name.contains('/') || name.contains(char::is_whitespace) {
            return Err(format!("{name:?} is not a bare entry name"));
        }
        if !seen.insert(name) {
            return Err(format!("{name} is listed twice"));
        }
        out.push(name);
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// `run` removes it from a session's environment (§12.5).
    Scrub,
    /// Classified as harmless: `run` passes it through.
    Known,
}

/// `known-env`: `<class> <NAME>` per entry. A name is upper-case ASCII letters, digits, `_` and
/// `*` globs, and is listed once.
fn known_env(text: &str) -> Result<Vec<(Class, &str)>, String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for line in entries(text) {
        let (class, name) = line
            .split_once(char::is_whitespace)
            .ok_or_else(|| format!("{line:?} has no class"))?;
        let class = match class {
            "scrub" => Class::Scrub,
            "known" => Class::Known,
            other => return Err(format!("unknown class {other:?} in {line:?}")),
        };
        let name = name.trim();
        let valid = |b: u8| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_' || b == b'*';
        if name.is_empty() || !name.bytes().all(valid) {
            return Err(format!("{name:?} is not an environment name"));
        }
        if !seen.insert(name) {
            return Err(format!("{name} is listed twice"));
        }
        out.push((class, name));
    }
    Ok(out)
}

fn names_of(env: &[(Class, &'static str)], class: Class) -> Vec<&'static str> {
    env.iter()
        .filter(|(c, _)| *c == class)
        .map(|(_, n)| *n)
        .collect()
}

#[test]
fn every_compat_file_parses() {
    list(KNOWN_SHARED).unwrap();
    list(KNOWN_PRIVATE).unwrap();
    known_env(KNOWN_ENV).unwrap();
    // Only that it parses: `cargo xtask compat --bless` advances it.
    assert!(tested_version(TESTED_CC_VERSION).is_some());
}

#[test]
fn the_readers_refuse_what_the_drift_script_refuses() {
    assert!(
        known_env("maybe CLAUDE_CODE_X").is_err(),
        "an unknown class"
    );
    assert!(known_env("CLAUDE_CODE_X").is_err(), "no class");
    assert!(known_env("scrub CLAUDE_CODE_X\nknown CLAUDE_CODE_X").is_err());
    assert!(known_env("known claude_code_lower").is_err());
    assert!(list("projects\nprojects").is_err());
    assert!(list("projects/").is_err());
    assert!(tested_version("2.1").is_none());
    assert!(tested_version("2.1.286\n2.1.287").is_none());
    assert!(tested_version("2.1.+3").is_none());
    assert_eq!(
        tested_version("# tested\n\n  2.1.286 \r\n"),
        Some((2, 1, 286))
    );
    assert_eq!(
        known_env("# c\n\n  known  CLAUDE_CODE_X \r\n").unwrap(),
        [(Class::Known, "CLAUDE_CODE_X")]
    );
}

#[test]
fn known_shared_is_the_share_allowlist_must_share_first() {
    let want: Vec<&str> = CC_MUST_SHARE
        .iter()
        .map(|(name, _)| *name)
        .chain(CC_SHARED.iter().copied())
        .collect();
    assert_eq!(list(KNOWN_SHARED).unwrap(), want);
}

#[test]
fn known_private_is_the_known_private_list() {
    assert_eq!(list(KNOWN_PRIVATE).unwrap(), CC_PRIVATE);
}

#[test]
fn no_shared_entry_is_also_private() {
    let private = list(KNOWN_PRIVATE).unwrap();
    for shared in list(KNOWN_SHARED).unwrap() {
        assert!(
            !private.iter().any(|p| entry_matches(p, shared)),
            "{shared} is on both lists"
        );
    }
}

#[test]
fn known_env_scrubs_exactly_what_run_scrubs() {
    let env = known_env(KNOWN_ENV).unwrap();
    let (globs, names): (Vec<&str>, Vec<&str>) = names_of(&env, Class::Scrub)
        .into_iter()
        .partition(|n| n.contains('*'));
    assert_eq!(
        names.into_iter().collect::<BTreeSet<_>>(),
        CC_SCRUB.iter().copied().collect::<BTreeSet<_>>()
    );
    assert_eq!(globs, ["CLAUDE_CODE_*_FILE_DESCRIPTOR"]);
    // The glob covers exactly the descriptors `session_env` expands at a launch.
    for name in [
        "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
        "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
        "CLAUDE_CODE__FILE_DESCRIPTOR",
        "CLAUDE_CODE_FILE_DESCRIPTOR",
        "XCLAUDE_CODE_A_FILE_DESCRIPTOR",
        "CLAUDE_CODE_A_FILE_DESCRIPTOR_X",
    ] {
        let scrubbed = session_env("/p", [OsString::from(name)])
            .remove
            .contains(&OsString::from(name));
        assert_eq!(entry_matches(globs[0], name), scrubbed, "{name}");
    }
}

#[test]
fn every_known_name_is_one_the_drift_job_meets_and_none_is_scrubbed() {
    let env = known_env(KNOWN_ENV).unwrap();
    let scrub = names_of(&env, Class::Scrub);
    for name in names_of(&env, Class::Known) {
        assert!(
            FAMILIES.iter().any(|f| name.starts_with(f)),
            "{name} is in neither family, so the drift job never meets it"
        );
        assert!(
            !scrub.iter().any(|s| entry_matches(s, name)),
            "{name} is both known and scrubbed"
        );
    }
}

/// The known patterns of `text` that match a literal scrub name.
fn known_over_scrub(text: &str) -> Vec<&str> {
    known_env(text)
        .unwrap()
        .into_iter()
        .filter(|(class, _)| *class == Class::Known)
        .map(|(_, name)| name)
        .filter(|k| CC_SCRUB.iter().any(|s| entry_matches(k, s)))
        .collect()
}

#[test]
fn no_known_pattern_matches_a_scrub_name() {
    assert_eq!(known_over_scrub(KNOWN_ENV), Vec::<&str>::new());
    let widened = format!("known CLAUDE_CODE_OAUTH_*\n{KNOWN_ENV}");
    assert_eq!(known_over_scrub(&widened), ["CLAUDE_CODE_OAUTH_*"]);
}
