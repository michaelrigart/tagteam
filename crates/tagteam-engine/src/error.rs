use std::io;

use tagteam_provider::{LockError, ProviderError, ReadError};

use crate::store::StoreError;
use crate::vault::VaultError;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error("{0}")]
    Unreadable(ReadError),
    #[error("unknown provider {0:?}")]
    UnknownProvider(String),
    #[error("this command cannot run inside a `tagteam run` session")]
    InsideRunShell,
    #[error("there is no live login to add; log in with `claude` first")]
    NoLiveLogin,
    #[error("the live login is a managed API key; add it with `tagteam add-token`")]
    LiveApiKey,
    #[error(
        "the Keychain could not be read, so the live credential may be out of date; unlock the Keychain and retry"
    )]
    DegradedRead,
    #[error("the live credential belongs to {found}, not {expected}; refusing to add it")]
    OwnerMismatch { expected: String, found: String },
    #[error("the live login changed while it was being checked; run the command again")]
    LiveMoved,
    #[error("position {position} holds {occupant}; confirm to replace it, or pass --yes")]
    NeedsConfirmation { position: u32, occupant: String },
    #[error(
        "the live login for {label} belongs to a different account than the stored {label} \
         (for example a reused email); remove the stored account before adding this login"
    )]
    IdentityConflict { label: String },
    #[error("{0}")]
    InvalidInput(String),
    #[error("no account matches {0:?}")]
    NoSuchAccount(String),
    #[error("{input:?} matches several accounts: {}", candidates.join(", "))]
    Ambiguous {
        input: String,
        candidates: Vec<String>,
    },
    #[error(
        "an interrupted switch for {0} could not be resolved; run `tagteam switch <account> --force` to settle it"
    )]
    InterruptedSwitch(String),
    #[error("the switch failed and was rolled back: {0}")]
    RolledBack(String),
    #[error("the switch failed ({cause}) and rolling back also failed: {failed}")]
    RollbackFailed { cause: String, failed: String },
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl EngineError {
    /// Stable `error.type` for `--json` output (§14).
    pub fn kind(&self) -> &'static str {
        match self {
            EngineError::Store(_) => "store",
            EngineError::Provider(ProviderError::ConfigUnsplicable(_)) => "config-unsplicable",
            EngineError::Provider(ProviderError::Lock(LockError::Timeout(_))) => "lock-timeout",
            EngineError::Provider(ProviderError::RestoreFailed { .. }) => "rollback-failed",
            EngineError::Provider(_) => "provider",
            EngineError::Lock(LockError::Timeout(_)) => "lock-timeout",
            EngineError::Lock(_) => "lock",
            EngineError::Vault(_) => "vault",
            EngineError::Unreadable(_) => "unreadable",
            EngineError::UnknownProvider(_) => "unknown-provider",
            EngineError::InsideRunShell => "inside-run-shell",
            EngineError::NoLiveLogin => "no-live-login",
            EngineError::LiveApiKey => "live-api-key",
            EngineError::DegradedRead => "degraded-read",
            EngineError::OwnerMismatch { .. } => "owner-mismatch",
            EngineError::LiveMoved => "live-moved",
            EngineError::NeedsConfirmation { .. } => "needs-confirmation",
            EngineError::IdentityConflict { .. } => "identity-conflict",
            EngineError::InvalidInput(_) => "invalid-input",
            EngineError::NoSuchAccount(_) => "no-such-account",
            EngineError::Ambiguous { .. } => "ambiguous-account",
            EngineError::InterruptedSwitch(_) => "interrupted-switch",
            EngineError::RolledBack(_) => "rolled-back",
            EngineError::RollbackFailed { .. } => "rollback-failed",
            EngineError::Io(_) => "io",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// Pins every variant's `kind()` string: it's part of `--json`'s stable contract (§14), so
    /// a change here is a breaking change, not a refactor.
    #[test]
    fn kind_is_pinned_for_every_variant() {
        let cases: Vec<(EngineError, &str)> = vec![
            (EngineError::Store(StoreError::NoSuchAccount), "store"),
            (
                EngineError::Provider(ProviderError::ConfigUnsplicable(PathBuf::from("x"))),
                "config-unsplicable",
            ),
            (
                EngineError::Provider(ProviderError::Lock(LockError::Timeout(PathBuf::from("x")))),
                "lock-timeout",
            ),
            (
                EngineError::Provider(ProviderError::RestoreFailed {
                    cause: Box::new(ProviderError::Invalid("a".into())),
                    restore: Box::new(ProviderError::Invalid("b".into())),
                }),
                "rollback-failed",
            ),
            (
                EngineError::Provider(ProviderError::Invalid("x".into())),
                "provider",
            ),
            (
                EngineError::Lock(LockError::Timeout(PathBuf::from("x"))),
                "lock-timeout",
            ),
            (
                EngineError::Lock(LockError::Compromised(PathBuf::from("x"))),
                "lock",
            ),
            (EngineError::Vault(VaultError::Verify), "vault"),
            (
                EngineError::Unreadable(ReadError::new("k", "d")),
                "unreadable",
            ),
            (EngineError::UnknownProvider("p".into()), "unknown-provider"),
            (EngineError::InsideRunShell, "inside-run-shell"),
            (EngineError::NoLiveLogin, "no-live-login"),
            (EngineError::LiveApiKey, "live-api-key"),
            (EngineError::DegradedRead, "degraded-read"),
            (
                EngineError::OwnerMismatch {
                    expected: "a".into(),
                    found: "b".into(),
                },
                "owner-mismatch",
            ),
            (EngineError::LiveMoved, "live-moved"),
            (
                EngineError::NeedsConfirmation {
                    position: 1,
                    occupant: "a".into(),
                },
                "needs-confirmation",
            ),
            (
                EngineError::IdentityConflict { label: "a".into() },
                "identity-conflict",
            ),
            (EngineError::InvalidInput("x".into()), "invalid-input"),
            (EngineError::NoSuchAccount("x".into()), "no-such-account"),
            (
                EngineError::Ambiguous {
                    input: "x".into(),
                    candidates: vec!["a".into()],
                },
                "ambiguous-account",
            ),
            (
                EngineError::InterruptedSwitch("p".into()),
                "interrupted-switch",
            ),
            (EngineError::RolledBack("x".into()), "rolled-back"),
            (
                EngineError::RollbackFailed {
                    cause: "a".into(),
                    failed: "b".into(),
                },
                "rollback-failed",
            ),
            (EngineError::Io(io::Error::other("x")), "io"),
        ];
        for (err, want) in cases {
            assert_eq!(err.kind(), want, "{err:?}");
        }
    }
}
