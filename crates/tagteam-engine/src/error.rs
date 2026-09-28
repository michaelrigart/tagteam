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
