//! Appendix A.5. The production URLs are the only ones a release build uses; the CLI's
//! test-support build points them at a local server (`with_base`).

/// Claude Code's OAuth client id, sent with every token refresh (Appendix A.5).
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub token: String,
    pub profile: String,
    pub usage: String,
}

impl Endpoints {
    pub fn production() -> Self {
        Self {
            token: "https://platform.claude.com/v1/oauth/token".into(),
            profile: "https://api.anthropic.com/api/oauth/profile".into(),
            usage: "https://api.anthropic.com/api/oauth/usage".into(),
        }
    }

    /// Every endpoint under one base URL, with the production paths.
    pub fn with_base(base: &str) -> Self {
        let b = base.trim_end_matches('/');
        Self {
            token: format!("{b}/v1/oauth/token"),
            profile: format!("{b}/api/oauth/profile"),
            usage: format!("{b}/api/oauth/usage"),
        }
    }
}
