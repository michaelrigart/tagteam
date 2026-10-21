use std::borrow::Cow;

use tagteam_provider::TransientKind;

/// `usage_state.last_error`'s kind token (§8.3, §6.1): what a usage collection recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureKind {
    NoAccessToken,
    Http401,
    RefreshFailed,
    KeychainUnavailable,
    ProfileDrifted,
    LiveReplaced,
    ForeignCredential,
    VaultAbsent,
    TokenExpired,
    OverBudget,
    /// Spelled by `TransientKind::token`. `From` and `parse` never build `Http(401)` here.
    Provider(TransientKind),
    /// A token this build does not know, read back from the store as it was.
    Other(String),
}

impl FailureKind {
    /// The tokens with a variant of their own, so each spelling is written once, in `as_str`.
    const NAMED: [FailureKind; 10] = [
        FailureKind::NoAccessToken,
        FailureKind::Http401,
        FailureKind::RefreshFailed,
        FailureKind::KeychainUnavailable,
        FailureKind::ProfileDrifted,
        FailureKind::LiveReplaced,
        FailureKind::ForeignCredential,
        FailureKind::VaultAbsent,
        FailureKind::TokenExpired,
        FailureKind::OverBudget,
    ];

    /// The stored spelling (§6.1).
    pub fn as_str(&self) -> Cow<'_, str> {
        Cow::Borrowed(match self {
            FailureKind::NoAccessToken => "no-access-token",
            FailureKind::Http401 => "http-401",
            FailureKind::RefreshFailed => "refresh-failed",
            FailureKind::KeychainUnavailable => "keychain-unavailable",
            FailureKind::ProfileDrifted => "profile-drifted",
            FailureKind::LiveReplaced => "live-replaced",
            FailureKind::ForeignCredential => "foreign-credential",
            FailureKind::VaultAbsent => "vault-absent",
            FailureKind::TokenExpired => "token-expired",
            FailureKind::OverBudget => "over-budget",
            FailureKind::Provider(kind) => return Cow::Owned(kind.token()),
            FailureKind::Other(token) => token,
        })
    }

    /// Reads a stored token: a named one, else a provider's, else `Other` as it was. Total, and
    /// `parse(s).as_str() == s` for every `s`.
    pub fn parse(s: &str) -> FailureKind {
        Self::NAMED
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .or_else(|| TransientKind::parse(s).map(FailureKind::from))
            .unwrap_or_else(|| FailureKind::Other(s.to_owned()))
    }
}

impl From<TransientKind> for FailureKind {
    /// `Http(401)` becomes `Http401`, so one token has one variant.
    fn from(kind: TransientKind) -> Self {
        match kind {
            TransientKind::Http(401) => FailureKind::Http401,
            kind => FailureKind::Provider(kind),
        }
    }
}

/// Compares the stored spelling: `kind == "http-429"` holds for the kind `parse("http-429")` gives.
impl PartialEq<str> for FailureKind {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

/// Why the refresh gate (§7.3) or active-token refresh (§7.5) did nothing decisive (§8.6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateFailure {
    VaultAbsent,
    NotRefreshable,
    VaultUnreadable,
    RescueUnreadable,
    ProfileUnreadable,
    RefreshFailed,
    VaultWrite,
    /// The token request itself failed, spelled by `TransientKind::token`.
    Provider(TransientKind),
}

impl GateFailure {
    /// The spelling the logs, the warnings and `AutoFreshened::Failed` carry.
    pub fn as_str(&self) -> Cow<'static, str> {
        Cow::Borrowed(match self {
            GateFailure::VaultAbsent => "vault-absent",
            GateFailure::NotRefreshable => "not-refreshable",
            GateFailure::VaultUnreadable => "vault-unreadable",
            GateFailure::RescueUnreadable => "rescue-unreadable",
            GateFailure::ProfileUnreadable => "profile-unreadable",
            GateFailure::RefreshFailed => "refresh-failed",
            GateFailure::VaultWrite => "vault-write",
            GateFailure::Provider(kind) => return Cow::Owned(kind.token()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_kind_keeps_its_stored_spelling() {
        use FailureKind::*;
        for (kind, s) in [
            (NoAccessToken, "no-access-token"),
            (Http401, "http-401"),
            (RefreshFailed, "refresh-failed"),
            (KeychainUnavailable, "keychain-unavailable"),
            (ProfileDrifted, "profile-drifted"),
            (LiveReplaced, "live-replaced"),
            (ForeignCredential, "foreign-credential"),
            (VaultAbsent, "vault-absent"),
            (TokenExpired, "token-expired"),
            (OverBudget, "over-budget"),
            (Provider(TransientKind::PreSend), "pre-send"),
            (Provider(TransientKind::Ambiguous), "ambiguous"),
            (Provider(TransientKind::BadResponse), "bad-response"),
            (Provider(TransientKind::Http(429)), "http-429"),
            (Other("x-new".into()), "x-new"),
        ] {
            assert_eq!(kind.as_str(), s);
            assert_eq!(FailureKind::parse(s), kind, "{s}");
        }
    }

    #[test]
    fn parse_reads_back_exactly_what_was_stored() {
        for s in [
            "",
            "error",
            "no-data",
            "http-0401",
            "HTTP-401",
            "over_budget",
            " over-budget",
        ] {
            let kind = FailureKind::parse(s);
            assert_eq!(kind, FailureKind::Other(s.to_owned()), "{s:?}");
            assert_eq!(kind.as_str(), s);
        }
    }

    #[test]
    fn a_transient_401_is_the_http_401_token() {
        assert_eq!(
            FailureKind::from(TransientKind::Http(401)),
            FailureKind::Http401
        );
        assert_eq!(FailureKind::parse("http-401"), FailureKind::Http401);
        assert_eq!(
            FailureKind::from(TransientKind::Http(429)),
            FailureKind::Provider(TransientKind::Http(429))
        );
    }

    #[test]
    fn a_kind_equals_its_stored_spelling_only() {
        let kind = &FailureKind::from(TransientKind::Http(429));
        assert!(kind == "http-429");
        assert!(kind != "http-401");
        assert!(&FailureKind::Other("x-new".into()) == "x-new");
    }

    #[test]
    fn every_gate_failure_keeps_its_spelling() {
        use GateFailure::*;
        for (kind, s) in [
            (VaultAbsent, "vault-absent"),
            (NotRefreshable, "not-refreshable"),
            (VaultUnreadable, "vault-unreadable"),
            (RescueUnreadable, "rescue-unreadable"),
            (ProfileUnreadable, "profile-unreadable"),
            (RefreshFailed, "refresh-failed"),
            (VaultWrite, "vault-write"),
            (Provider(TransientKind::PreSend), "pre-send"),
            (Provider(TransientKind::Http(503)), "http-503"),
        ] {
            assert_eq!(kind.as_str(), s);
        }
    }
}
