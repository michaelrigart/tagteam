//! Claude Code's version: what `claude --version` reports, what `compat/tested-cc-version`
//! records, and `--bless`, the only way the tested version advances (§15.4).

use std::fmt;

/// `MAJOR.MINOR.PATCH`, ordered numerically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CcVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl fmt::Display for CcVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl CcVersion {
    /// Three dot-separated decimal numbers and nothing else.
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.split('.').map(|p| {
            (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse::<u32>().ok())
                .flatten()
        });
        let version = Self {
            major: parts.next()??,
            minor: parts.next()??,
            patch: parts.next()??,
        };
        parts.next().is_none().then_some(version)
    }

    /// `claude --version`'s first line, `MAJOR.MINOR.PATCH (Claude Code)`, as the weekly job
    /// reads it (`scripts/cc-drift.sh`).
    pub fn from_claude_output(out: &str) -> Result<Self, String> {
        let line = out.lines().next().unwrap_or("").trim_end_matches('\r');
        line.strip_suffix(" (Claude Code)")
            .and_then(Self::parse)
            .ok_or_else(|| {
                format!("`claude --version` printed {line:?}, not `X.Y.Z (Claude Code)`")
            })
    }

    /// `compat/tested-cc-version`: one version line; blank lines and `#` comments ignored.
    pub fn from_tested_file(text: &str) -> Result<Self, String> {
        match version_lines(text)[..] {
            [(_, line)] => Self::parse(line.trim())
                .ok_or_else(|| format!("tested-cc-version holds {line:?}, not X.Y.Z")),
            _ => Err("tested-cc-version must hold exactly one version line".into()),
        }
    }
}

/// The lines of `text` that are neither blank nor comments, with their indices.
fn version_lines(text: &str) -> Vec<(usize, &str)> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| {
            let l = l.trim();
            !l.is_empty() && !l.starts_with('#')
        })
        .collect()
}

/// `text`, the tested-version file, with its version line replaced by `v`. Every other line,
/// the comments included, is kept as it was.
pub fn blessed(text: &str, v: CcVersion) -> Result<String, String> {
    CcVersion::from_tested_file(text)?;
    let (index, _) = version_lines(text)[0];
    let mut out: Vec<String> = text.lines().map(str::to_owned).collect();
    out[index] = v.to_string();
    let mut joined = out.join("\n");
    joined.push('\n');
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TESTED: &str = "# The newest Claude Code release tagteam has been checked against\n# (spec 13.6, TESTED_CC_VERSION). One MAJOR.MINOR.PATCH line.\n2.1.286\n";

    fn v(major: u32, minor: u32, patch: u32) -> CcVersion {
        CcVersion {
            major,
            minor,
            patch,
        }
    }

    #[test]
    fn claude_s_version_line_parses_and_nothing_else_does() {
        assert_eq!(
            CcVersion::from_claude_output("2.1.286 (Claude Code)\n").unwrap(),
            v(2, 1, 286)
        );
        assert_eq!(
            CcVersion::from_claude_output("2.10.3 (Claude Code)\r\nmore\n").unwrap(),
            v(2, 10, 3)
        );
        for bad in [
            "",
            "2.1.286",
            "2.1 (Claude Code)",
            "2.1.286.1 (Claude Code)",
            "v2.1.286 (Claude Code)",
            "2.1.+6 (Claude Code)",
            "2.1.286 (Claude Code) beta",
        ] {
            assert!(CcVersion::from_claude_output(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn versions_order_numerically() {
        assert!(v(2, 1, 287) > v(2, 1, 286));
        assert!(v(2, 10, 0) > v(2, 9, 99));
        assert!(v(3, 0, 0) > v(2, 99, 99));
    }

    #[test]
    fn the_tested_file_holds_one_version_among_comments() {
        assert_eq!(CcVersion::from_tested_file(TESTED).unwrap(), v(2, 1, 286));
        assert!(CcVersion::from_tested_file("# only a comment\n").is_err());
        assert!(CcVersion::from_tested_file("2.1.286\n2.1.287\n").is_err());
        assert!(CcVersion::from_tested_file("2.1\n").is_err());
    }

    #[test]
    fn blessing_replaces_the_version_line_and_keeps_the_comments() {
        let out = blessed(TESTED, v(2, 1, 290)).unwrap();
        assert_eq!(out, TESTED.replace("2.1.286", "2.1.290"));
        assert_eq!(CcVersion::from_tested_file(&out).unwrap(), v(2, 1, 290));
        assert!(blessed("# nothing\n", v(2, 1, 290)).is_err());
    }

    #[test]
    fn the_committed_tested_version_parses() {
        let text = include_str!("../../../tagteam-cc/compat/tested-cc-version");
        CcVersion::from_tested_file(text).unwrap();
    }
}
