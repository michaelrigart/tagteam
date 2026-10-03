use std::io;
use std::path::{Path, PathBuf};

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
    /// A vault that could not be read where the switch needed it: a direct target, or an
    /// account a rotation met before its pick, which could have been the pick (§9.3). It is
    /// named, never skipped and never taken for a missing credential.
    #[error("the stored credential for {label} (position {position}) is unreadable: {source}")]
    UnreadableAccount {
        position: u32,
        label: String,
        source: ReadError,
    },
    #[error("unknown provider {0:?}")]
    UnknownProvider(String),
    #[error("this command cannot run inside a `tagteam run` session")]
    InsideRunShell,
    /// §12.8: a marker is present but is not a valid marker, so the outer home is unknown.
    /// Every command but `statusline` refuses, naming it.
    #[error("the run-shell marker {} cannot be read ({detail})", marker.display())]
    RunShellUnreadable { marker: PathBuf, detail: String },
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
    /// A target whose stored refresh token can no longer be used: rejected (§7.4, it is
    /// quarantined), or spent by a refresh whose successor could be stored nowhere (§7.3
    /// step 6, `Unpersisted`). Only a new login brings it back.
    #[error(
        "{label} (position {position}) needs a new login: its stored refresh token can no longer be used; log in with `claude`, then run `tagteam add`"
    )]
    NeedsRelogin { position: u32, label: String },
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
    /// An interrupted switch whose recovery could not take the provider's live locks: not an
    /// undecidable row, so a retry recovers it and `--force` is not the way out.
    #[error(
        "an interrupted switch for {provider} could not be recovered yet: timed out waiting for the lock {}; retry once {app} is idle",
        lock.display()
    )]
    RecoveryBlocked {
        provider: String,
        app: &'static str,
        lock: PathBuf,
    },
    /// An interrupted switch whose recovery found its agent writing the credential entry it was
    /// clearing (§9.1's `EntryMoved`): nothing was cleared and the row stays, so a plain retry
    /// settles it. Not an undecidable row, so `--force` is not the way out either.
    #[error(
        "an interrupted switch for {provider} could not be recovered yet: {app} changed its credential while it was being recovered; retry to settle it"
    )]
    RecoveryMoved { provider: String, app: &'static str },
    /// §6.2: a refreshed successor sits in `rescue/` and could not be adopted, or a rescue file
    /// for the account cannot be read. Activating or refreshing the vault's generation would
    /// use a token the server has already consumed.
    #[error(
        "{label} (position {position}) has a refreshed token that is not in the vault yet: {detail}; retry once the vault can be written"
    )]
    RescuePending {
        position: u32,
        label: String,
        detail: String,
    },
    #[error("the switch failed and was rolled back: {0}")]
    RolledBack(String),
    #[error("the switch failed ({cause}) and rolling back also failed: {failed}")]
    RollbackFailed { cause: String, failed: String },
    /// §7.5: the oracle attributed the live credential to another identity, so tagteam neither
    /// adopts nor refreshes it. `usageStatus` calls this `foreign_credential` (§13.2).
    #[error(
        "the live credential does not belong to the account at position {position}; tagteam will not refresh it"
    )]
    ForeignLiveCredential { position: u32 },
    /// §9.2, §10.3: the account is session-owned (§12.5). Activating it would give the default
    /// home and the session two copies of one single-use refresh token; destroying it would
    /// pull the login from under a running session. `unreadable` is set when a reservation or a
    /// session record could not be read (§12.6), which counts as owned: it names the file and
    /// why, since nothing may be running at all and the user has a file to repair.
    #[error("{}", session_owned_message(*.position, .label, .unreadable.as_deref()))]
    SessionOwned {
        position: u32,
        label: String,
        unreadable: Option<String>,
    },
    /// §9.2, §12.5: the account's quiescent session profile and the vault both moved since they
    /// last agreed, so the vault's generation may be consumed. An explicit replacement resolves
    /// it.
    #[error(
        "position {position} ({label})'s session profile and the vault both moved since they last agreed; log in again with `tagteam add` to resolve it"
    )]
    ProfileConflict { position: u32, label: String },
    /// §12.2: the profile holds a must-share entry as a real copy, or as a link that resolves
    /// elsewhere, where tagteam's link to the shared one belongs. Splitting memory or history
    /// silently is never an option, so the launch refuses until the user merges the two, or,
    /// when tagteam's own link went stale under a running session, until that session ends.
    #[error("{}", split_message(.profile, .shared, *.cause))]
    ProfileSplit {
        profile: PathBuf,
        shared: PathBuf,
        cause: SplitCause,
    },
    /// §12.1: the provider's launch command is not on `PATH`; `run` changed nothing.
    #[error("`{command}` is not on PATH; install it, or add its directory to PATH")]
    LaunchCommandMissing { command: String },
    /// §12.1: an API-key account has no login a session could run in a profile of its own.
    #[error(
        "position {position} is an API-key account, which `tagteam run` cannot start a session for; `tagteam switch {position}` makes it the live login"
    )]
    ApiKeyAccount { position: u32 },
    /// §12.1 `--require-session`: `run` would have run plain `claude` instead of a session.
    #[error("--require-session: {why}, so no session would start")]
    RequiresSession { why: String },
    #[error(transparent)]
    Io(#[from] io::Error),
    /// §14.1: a cancellation point outside a lock wait found the cancel token set. A lock wait
    /// reports its own `LockError::Interrupted`; `signal()` reads either.
    #[error("interrupted")]
    Interrupted(i32),
}

/// Why a profile's must-share entry is split from the shared one (§12.2), which decides what
/// the user does about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitCause {
    /// A real file or directory in the profile.
    RealCopy,
    /// A link tagteam did not make, which resolves somewhere other than the shared entry.
    LinkElsewhere,
    /// tagteam's own link, which no longer resolves where the shared entry does while a session
    /// still runs in the profile: a join never changes the links a running session uses.
    StaleWhileRunning,
}

/// `ProfileSplit`'s message, by cause.
fn split_message(profile: &Path, shared: &Path, cause: SplitCause) -> String {
    let (profile, shared) = (profile.display(), shared.display());
    match cause {
        SplitCause::RealCopy => format!(
            "{profile} is a real copy where {shared} should be linked; merge the two by hand, then remove the copy"
        ),
        SplitCause::LinkElsewhere => format!(
            "{profile} links somewhere other than {shared}, where it should link; merge the two by hand, then remove the link"
        ),
        SplitCause::StaleWhileRunning => format!(
            "{profile} still links where {shared} used to be, and a session in the profile uses that link; end that session, then launch again"
        ),
    }
}

/// `SessionOwned`'s message: a running session, or session state that cannot be read.
fn session_owned_message(position: u32, label: &str, unreadable: Option<&str>) -> String {
    match unreadable {
        None => format!(
            "position {position} ({label}) is in use by a `tagteam run` session; exit that session first"
        ),
        Some(detail) => format!(
            "position {position} ({label}) counts as in use by a `tagteam run` session because its session state cannot be read ({detail}); repair or remove that file, then retry"
        ),
    }
}

impl EngineError {
    /// Stable `error.type` for `--json` output (§14).
    pub fn kind(&self) -> &'static str {
        match self {
            EngineError::Store(_) => "store",
            EngineError::Provider(ProviderError::ConfigUnsplicable { .. }) => "config-unsplicable",
            EngineError::Provider(ProviderError::Lock(LockError::Timeout(_))) => "lock-timeout",
            EngineError::Provider(ProviderError::Lock(LockError::Interrupted { .. })) => {
                "interrupted"
            }
            EngineError::Provider(ProviderError::RestoreFailed { .. }) => "rollback-failed",
            EngineError::Provider(_) => "provider",
            EngineError::Lock(LockError::Timeout(_)) => "lock-timeout",
            EngineError::Lock(LockError::Interrupted { .. }) => "interrupted",
            EngineError::Lock(_) => "lock",
            EngineError::Vault(_) => "vault",
            EngineError::Unreadable(_) => "unreadable",
            EngineError::UnreadableAccount { .. } => "unreadable",
            EngineError::UnknownProvider(_) => "unknown-provider",
            EngineError::InsideRunShell => "inside-run-shell",
            EngineError::RunShellUnreadable { .. } => "run-shell-unreadable",
            EngineError::NoLiveLogin => "no-live-login",
            EngineError::LiveApiKey => "live-api-key",
            EngineError::DegradedRead => "degraded-read",
            EngineError::OwnerMismatch { .. } => "owner-mismatch",
            EngineError::LiveMoved => "live-moved",
            EngineError::NeedsConfirmation { .. } => "needs-confirmation",
            EngineError::IdentityConflict { .. } => "identity-conflict",
            EngineError::NeedsRelogin { .. } => "relogin-required",
            EngineError::InvalidInput(_) => "invalid-input",
            EngineError::NoSuchAccount(_) => "no-such-account",
            EngineError::Ambiguous { .. } => "ambiguous-account",
            EngineError::InterruptedSwitch(_) => "interrupted-switch",
            EngineError::RecoveryBlocked { .. } | EngineError::RecoveryMoved { .. } => {
                "interrupted-switch"
            }
            EngineError::RescuePending { .. } => "rescue-pending",
            EngineError::RolledBack(_) => "rolled-back",
            EngineError::RollbackFailed { .. } => "rollback-failed",
            EngineError::ForeignLiveCredential { .. } => "foreign-credential",
            EngineError::SessionOwned { .. } => "session-owned",
            EngineError::ProfileConflict { .. } => "profile-conflict",
            EngineError::ProfileSplit { .. } => "profile-split",
            EngineError::LaunchCommandMissing { .. } => "launch-command-missing",
            EngineError::ApiKeyAccount { .. } => "api-key-account",
            EngineError::RequiresSession { .. } => "requires-session",
            EngineError::Io(_) => "io",
            EngineError::Interrupted(_) => "interrupted",
        }
    }

    /// The signal behind an interruption, whichever carrier holds it: `Interrupted`,
    /// `Lock(LockError::Interrupted)`, or `Provider(ProviderError::Lock(LockError::Interrupted))`
    /// (§14.1). `None` for every other error.
    pub fn signal(&self) -> Option<i32> {
        match self {
            EngineError::Interrupted(signal) => Some(*signal),
            EngineError::Lock(e) | EngineError::Provider(ProviderError::Lock(e)) => e.signal(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins every variant's `kind()` string: it's part of `--json`'s stable contract (§14), so
    /// a change here is a breaking change, not a refactor.
    #[test]
    fn kind_is_pinned_for_every_variant() {
        let cases: Vec<(EngineError, &str)> = vec![
            (EngineError::Store(StoreError::NoSuchAccount), "store"),
            (
                EngineError::Provider(ProviderError::ConfigUnsplicable {
                    path: PathBuf::from("x"),
                    remedy: "r",
                }),
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
            (
                EngineError::UnreadableAccount {
                    position: 1,
                    label: "a".into(),
                    source: ReadError::new("k", "d"),
                },
                "unreadable",
            ),
            (EngineError::UnknownProvider("p".into()), "unknown-provider"),
            (EngineError::InsideRunShell, "inside-run-shell"),
            (
                EngineError::RunShellUnreadable {
                    marker: PathBuf::from("x"),
                    detail: "d".into(),
                },
                "run-shell-unreadable",
            ),
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
            (
                EngineError::NeedsRelogin {
                    position: 1,
                    label: "a@b.co".into(),
                },
                "relogin-required",
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
            (
                EngineError::RecoveryBlocked {
                    provider: "p".into(),
                    app: "a",
                    lock: PathBuf::from("x"),
                },
                "interrupted-switch",
            ),
            (
                EngineError::RecoveryMoved {
                    provider: "p".into(),
                    app: "a",
                },
                "interrupted-switch",
            ),
            (
                EngineError::RescuePending {
                    position: 1,
                    label: "a".into(),
                    detail: "d".into(),
                },
                "rescue-pending",
            ),
            (EngineError::RolledBack("x".into()), "rolled-back"),
            (
                EngineError::RollbackFailed {
                    cause: "a".into(),
                    failed: "b".into(),
                },
                "rollback-failed",
            ),
            (
                EngineError::ForeignLiveCredential { position: 1 },
                "foreign-credential",
            ),
            (
                EngineError::SessionOwned {
                    position: 1,
                    label: "a".into(),
                    unreadable: None,
                },
                "session-owned",
            ),
            (
                EngineError::SessionOwned {
                    position: 1,
                    label: "a".into(),
                    unreadable: Some("/p/sessions/7.json: not JSON".into()),
                },
                "session-owned",
            ),
            (
                EngineError::ProfileConflict {
                    position: 1,
                    label: "a".into(),
                },
                "profile-conflict",
            ),
            (
                EngineError::ProfileSplit {
                    profile: PathBuf::from("p"),
                    shared: PathBuf::from("s"),
                    cause: SplitCause::RealCopy,
                },
                "profile-split",
            ),
            (
                EngineError::ProfileSplit {
                    profile: PathBuf::from("p"),
                    shared: PathBuf::from("s"),
                    cause: SplitCause::LinkElsewhere,
                },
                "profile-split",
            ),
            (
                EngineError::ProfileSplit {
                    profile: PathBuf::from("p"),
                    shared: PathBuf::from("s"),
                    cause: SplitCause::StaleWhileRunning,
                },
                "profile-split",
            ),
            (
                EngineError::LaunchCommandMissing {
                    command: "claude".into(),
                },
                "launch-command-missing",
            ),
            (
                EngineError::ApiKeyAccount { position: 1 },
                "api-key-account",
            ),
            (
                EngineError::RequiresSession { why: "w".into() },
                "requires-session",
            ),
            (EngineError::Io(io::Error::other("x")), "io"),
            (
                EngineError::Provider(ProviderError::Lock(LockError::Interrupted {
                    path: PathBuf::from("x"),
                    signal: 2,
                })),
                "interrupted",
            ),
            (
                EngineError::Lock(LockError::Interrupted {
                    path: PathBuf::from("x"),
                    signal: 15,
                }),
                "interrupted",
            ),
            (EngineError::Interrupted(1), "interrupted"),
        ];
        for (err, want) in cases {
            assert_eq!(err.kind(), want, "{err:?}");
        }
    }

    #[test]
    fn signal_reads_every_carrier_of_an_interruption() {
        let lock = |signal| LockError::Interrupted {
            path: PathBuf::from("x"),
            signal,
        };
        assert_eq!(EngineError::Interrupted(2).signal(), Some(2));
        assert_eq!(EngineError::Lock(lock(15)).signal(), Some(15));
        assert_eq!(
            EngineError::Provider(ProviderError::Lock(lock(1))).signal(),
            Some(1)
        );
        assert_eq!(
            EngineError::Lock(LockError::Timeout(PathBuf::from("x"))).signal(),
            None
        );
        assert_eq!(
            EngineError::Provider(ProviderError::Lock(LockError::Compromised(PathBuf::from(
                "x"
            ))))
            .signal(),
            None
        );
        assert_eq!(EngineError::LiveMoved.signal(), None);
    }
}
