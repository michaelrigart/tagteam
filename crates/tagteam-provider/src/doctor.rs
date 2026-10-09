//! §13.6's result type, shared by the engine's checks and each provider's `doctor_checks`, so
//! the CLI renders one list (Decision 1).

/// How a check came out (§13.6). Ordered by severity, so the worst of a list is its maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CheckStatus {
    Ok,
    Info,
    Warn,
    Fail,
}

impl CheckStatus {
    /// The JSON `status` (§13.6).
    pub fn as_str(self) -> &'static str {
        match self {
            CheckStatus::Ok => "ok",
            CheckStatus::Info => "info",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
        }
    }
}

/// One finding (§13.6). `id` is a stable dotted name (`store.integrity`, `cc.version`, …);
/// several findings may share one, one per thing found. `fix` names what to do, and is `None`
/// when there is nothing to do. No message names an email, an organization or a secret: users
/// paste doctor's output into issues (§4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub id: String,
    pub status: CheckStatus,
    pub message: String,
    pub fix: Option<String>,
}

impl Check {
    pub fn new(id: impl Into<String>, status: CheckStatus, message: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            status,
            message: message.into(),
            fix: None,
        }
    }

    pub fn ok(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(id, CheckStatus::Ok, message)
    }

    pub fn info(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(id, CheckStatus::Info, message)
    }

    pub fn warn(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(id, CheckStatus::Warn, message)
    }

    pub fn fail(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(id, CheckStatus::Fail, message)
    }

    /// The same finding, naming what to do about it.
    pub fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

/// `path` in single quotes for a shell command a fix names, any `'` in it escaped.
pub fn quoted(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_worst_status_is_the_maximum() {
        let all = [
            CheckStatus::Info,
            CheckStatus::Fail,
            CheckStatus::Ok,
            CheckStatus::Warn,
        ];
        assert_eq!(all.iter().max(), Some(&CheckStatus::Fail));
        assert!(CheckStatus::Ok < CheckStatus::Info && CheckStatus::Info < CheckStatus::Warn);
        assert_eq!(
            all.map(CheckStatus::as_str),
            ["info", "fail", "ok", "warn"],
            "§13.6's spellings"
        );
    }

    #[test]
    fn a_check_names_its_fix_only_when_given_one() {
        let c = Check::warn("store.mode", "too open");
        assert_eq!(c.fix, None);
        let c = c.fix("chmod 600 'x'");
        assert_eq!(c.status, CheckStatus::Warn);
        assert_eq!(c.fix.as_deref(), Some("chmod 600 'x'"));
    }

    #[test]
    fn a_quoted_path_survives_a_single_quote() {
        assert_eq!(quoted(Path::new("/h/it's")), r"'/h/it'\''s'");
        assert_eq!(quoted(Path::new("/h/a b")), "'/h/a b'");
    }
}
