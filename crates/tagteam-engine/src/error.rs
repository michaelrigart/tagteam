use std::io;
use std::path::{Path, PathBuf};

use tagteam_provider::doctor::quoted;
use tagteam_provider::{LockError, ProviderError, ReadError};

use crate::session::{Damaged, DamagedKind, SessionState};
use crate::settings::SettingsError;
use crate::store::StoreError;
use crate::transfer::TransferError;
use crate::vault::VaultError;

/// " (from <source>)" when `claude auth status` named where the overriding key came from.
fn from_source(source: &Option<String>) -> String {
    source
        .as_deref()
        .map_or_else(String::new, |s| format!(" (from {s})"))
}

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
    /// §6.3: an ID `displaced --purge` was given that names no entry, or that is not a
    /// displaced ID at all (Decision 12).
    #[error("no displaced credential matches {0:?}; `tagteam displaced` lists them")]
    NoSuchDisplaced(String),
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
    /// `owner` says what owns it, and the message names each (§12.6).
    #[error("{}", session_owned_message(*.position, .label, .owner))]
    SessionOwned {
        position: u32,
        label: String,
        owner: Box<SessionOwner>,
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
    /// §12.3: the session would log in by another method than the account's own login.
    #[error(
        "position {position}'s session would log in by {method}{} rather than with its account's login; remove what sets it (an `apiKeyHelper` or `env` entry in the shared settings, the project's own settings, or a workload-identity profile), then run again",
        from_source(key_source)
    )]
    LoginOverridden {
        position: u32,
        method: String,
        key_source: Option<String>,
    },
    /// §12.3: logged out, or logged in as another account.
    #[error(
        "position {position}'s session would not be logged in as its account ({detail}); the next `tagteam run` sets its profile up afresh"
    )]
    LoginInvalid { position: u32, detail: String },
    /// §12.3: Claude Code resolves another config dir than the profile's recorded spelling.
    #[error(
        "position {position}'s session would use {reported} as its config dir, not its profile, so it was not started"
    )]
    LoginDrifted { position: u32, reported: String },
    /// §12.3: the check timed out, or its output did not parse.
    #[error("position {position}'s login could not be confirmed ({detail}), so it was not started")]
    LoginUnknown { position: u32, detail: String },
    /// §12.3: the launch command could not be spawned.
    #[error("the launch command could not be started: {detail}")]
    LaunchUnreachable { detail: String },
    /// §12.5 launch step 4: a live `<pid>.lock` holds this launch's reservation, left by an
    /// orphaned session of an earlier process with this pid (Task 4). `detail` names the file.
    /// Its kind is `launch-unreachable`, as the launch command's own failure: nothing started.
    #[error(
        "the profile could not be reserved for this launch: {detail}, whose session may still be running; run it again, as a new process with a pid of its own"
    )]
    ReservationHeld { detail: String },
    /// §12.5 launch step 1, B.47: under the launch's locks the target is no longer one a session
    /// may start for. It was removed (Decision 7), or it became the live default login. The
    /// CLI prints `why` and plans again, which runs plain `claude` or refuses under
    /// `--require-session`.
    #[error("{why}")]
    TargetChanged { why: String },
    /// A `config` command's refusal, or its failure to write (§6.4).
    #[error(transparent)]
    Settings(#[from] SettingsError),
    /// §12.5: a replacement landed (the vault holds `replacing_fp`), but what it recorded about
    /// the new login cannot be read, so it can be neither installed nor undone. Every holder of
    /// the account's lock refuses, except `remove` and `purge`, which delete it either way.
    #[error(
        "position {position} ({label}) has a new login whose recorded details cannot be read, so it cannot be finished; run `tagteam remove {position}`, then add the login again"
    )]
    ReplacementUnreadable { position: u32, label: String },
    /// §6.3: the `rescue` path is not a directory or cannot be listed, so every account's
    /// rescues are unknown. `remove` and a `--provider` purge refuse rather than guess which
    /// entries were the account's; only a full purge deletes the path (§10.5).
    #[error(
        "{} cannot be listed ({detail}), so it may hold any account's refreshed tokens; fix or move it, or run a full `tagteam purge`, which deletes it",
        path.display()
    )]
    RescueUnlistable { path: PathBuf, detail: String },
    /// §10.5 step 3: a provider's auto-switch engine holds its engine lock; `pid` is the one
    /// its lock record names, when it can be read (§11.1).
    #[error(
        "an auto-switch engine for {provider} is running{}; stop it, then run `tagteam purge` again",
        pid.map_or_else(String::new, |pid| format!(" (pid {pid})"))
    )]
    EngineRunning { provider: String, pid: Option<u32> },
    /// §10.5 step 6: a session profile that no store account owns is in use: a live launch
    /// reservation, or a session record that is live or cannot be read (§12.5, §12.6).
    /// `owner` says what is using it (§12.6).
    #[error("{}", orphan_message(profile, .owner))]
    OrphanSessionRunning {
        profile: PathBuf,
        owner: Box<SessionOwner>,
    },
    /// §10.5 step 6: the accounts a purge would delete are not the ones that were confirmed.
    #[error(
        "the accounts changed since the purge was confirmed (another command added or removed one); run `tagteam purge` again"
    )]
    PurgeChanged,
    /// §13.3: an account an explicit `--account` named whose exportable generation cannot be
    /// determined, or is known to be dead. Nothing is written.
    #[error("position {position} cannot be exported: {reason}")]
    AccountBroken { position: u32, reason: String },
    /// §13.3: an export or import file, its encryption, or a key for it.
    #[error(transparent)]
    Transfer(#[from] TransferError),
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

/// How to end a Claude Code background daemon that owns `profile` (§12.6, Appendix A.7), by the
/// profile path where tagteam found its `daemon.lock`. If the pid in the lock was reused by an
/// unrelated `claude`, the daemon is not running and only the lock is left, which Claude Code's
/// own advice has the user delete.
pub(crate) fn daemon_advice(profile: &Path) -> String {
    format!(
        "stop it with `claude daemon stop --any` run with CLAUDE_CONFIG_DIR set to {}; if nothing is running at that pid, delete {}",
        quoted(profile),
        quoted(&profile.join("daemon.lock"))
    )
}

/// What owns a profile that is session-owned (§12.5, §12.6): every live owner and every
/// unreadable input found, not the first. The one source of every user-facing text about
/// ownership (refusals, the auto-switch reason, warnings, doctor's `sessions.*` lines).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionOwner {
    /// A `tagteam run` session, or any live owner that is not a background daemon.
    pub session: bool,
    /// A Claude Code background daemon owns it, in this profile.
    pub daemon: Option<PathBuf>,
    /// Every input that could not be read.
    pub damaged: Vec<Damaged>,
}

impl SessionOwner {
    /// The owners and unreadable inputs `state` found.
    pub fn of(state: &SessionState) -> Self {
        match state {
            SessionState::Owned {
                profile,
                session,
                daemon,
                damaged,
            } => Self {
                session: *session,
                daemon: daemon.then(|| profile.clone()),
                damaged: damaged.clone(),
            },
            SessionState::Unreadable { damaged, .. } => Self {
                damaged: damaged.clone(),
                ..Self::default()
            },
            _ => Self::default(),
        }
    }

    /// Whether anything live owns it, as opposed to nothing readable.
    fn live(&self) -> bool {
        self.session || self.daemon.is_some()
    }

    /// The unreadable inputs, each named by its quoted file and why: "'f' cannot be read (why)".
    pub fn damaged_list(&self) -> String {
        self.damaged
            .iter()
            .map(|d| format!("{} cannot be read ({})", quoted(&d.file), d.detail))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Who owns it, as a predicate: "is in use by a `tagteam run` session and by a Claude Code
    /// background daemon", and "also" the unreadable inputs, or, with nothing live, that it
    /// "counts as in use because" of them.
    pub fn who(&self) -> String {
        let live = match (self.session, self.daemon.is_some()) {
            (true, true) => {
                "is in use by a `tagteam run` session and by a Claude Code background daemon"
            }
            (true, false) => "is in use by a `tagteam run` session",
            (false, true) => "is in use by a Claude Code background daemon",
            (false, false) => "",
        };
        match (self.live(), self.damaged.is_empty()) {
            (true, true) => live.to_owned(),
            (true, false) => format!("{live}, and also {}", self.damaged_list()),
            (false, false) => format!("counts as in use because {}", self.damaged_list()),
            (false, true) => "is in use by a `tagteam run` session".to_owned(),
        }
    }

    /// What repairs each unreadable input, once each.
    pub fn repairs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for d in &self.damaged {
            let r = match d.kind {
                DamagedKind::SupervisorLock => {
                    "if nothing runs as Claude Code for that profile, delete the lock"
                }
                DamagedKind::Record => "repair or remove that record",
                DamagedKind::Profile | DamagedKind::Reservations | DamagedKind::RecordsDir => {
                    "make it readable again"
                }
            };
            if !out.iter().any(|o| o == r) {
                out.push(r.to_owned());
            }
        }
        out
    }

    /// What ends each owner, then what repairs each unreadable input.
    pub fn remedies(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.session || (!self.live() && self.damaged.is_empty()) {
            out.push("exit that session".to_owned());
        }
        if let Some(profile) = &self.daemon {
            out.push(daemon_advice(profile));
        }
        out.extend(self.repairs());
        out
    }

    /// The whole sentence about `subject`: who owns it and what to do, then retry.
    pub fn reason(&self, subject: &str) -> String {
        format!(
            "{subject} {}; {}, then retry",
            self.who(),
            self.remedies().join("; ")
        )
    }
}

/// `OrphanSessionRunning`'s message.
fn orphan_message(profile: &Path, owner: &SessionOwner) -> String {
    owner.reason(&format!(
        "{} belongs to no stored account, but it",
        profile.display()
    ))
}

/// `SessionOwned`'s message: a running session, a daemon, or state that cannot be read.
fn session_owned_message(position: u32, label: &str, owner: &SessionOwner) -> String {
    owner.reason(&format!("position {position} ({label})"))
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
            EngineError::Lock(e) | EngineError::Settings(SettingsError::Lock(e)) => lock_kind(e),
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
            EngineError::NoSuchDisplaced(_) => "no-such-displaced",
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
            EngineError::LoginOverridden { .. } => "login-overridden",
            EngineError::LoginInvalid { .. } => "login-invalid",
            EngineError::LoginDrifted { .. } => "login-drifted",
            EngineError::LoginUnknown { .. } => "login-unknown",
            EngineError::LaunchUnreachable { .. } | EngineError::ReservationHeld { .. } => {
                "launch-unreachable"
            }
            EngineError::TargetChanged { .. } => "target-changed",
            EngineError::Settings(
                SettingsError::UnknownKey(_)
                | SettingsError::NotPerProvider(_)
                | SettingsError::ProviderMismatch { .. }
                | SettingsError::Invalid { .. }
                | SettingsError::UnknownProvider(_),
            ) => "invalid-input",
            EngineError::Settings(SettingsError::Corrupt { .. }) => "settings-unreadable",
            EngineError::Settings(SettingsError::Io(_)) => "io",
            EngineError::ReplacementUnreadable { .. } => "replacement-unreadable",
            EngineError::RescueUnlistable { .. } => "rescue-unreadable",
            EngineError::EngineRunning { .. } => "engine-running",
            EngineError::OrphanSessionRunning { .. } => "session-owned",
            EngineError::PurgeChanged => "purge-changed",
            EngineError::AccountBroken { .. } => "account-broken",
            EngineError::Transfer(e) => e.kind(),
            EngineError::Io(_) => "io",
            EngineError::Interrupted(_) => "interrupted",
        }
    }

    /// The signal behind an interruption, whichever carrier holds it: `Interrupted`,
    /// `Lock(LockError::Interrupted)`, `Provider(ProviderError::Lock(LockError::Interrupted))`,
    /// or the settings lock's `Settings(SettingsError::Lock(LockError::Interrupted))` (§14.1).
    /// `None` for every other error.
    pub fn signal(&self) -> Option<i32> {
        match self {
            EngineError::Interrupted(signal) => Some(*signal),
            EngineError::Lock(e)
            | EngineError::Provider(ProviderError::Lock(e))
            | EngineError::Settings(SettingsError::Lock(e)) => e.signal(),
            _ => None,
        }
    }
}

/// A lock failure's kind, whichever lock it was: the settings lock's (`SettingsError::Lock`)
/// reads exactly as an engine lock's (Decision 4).
fn lock_kind(e: &LockError) -> &'static str {
    match e {
        LockError::Timeout(_) => "lock-timeout",
        LockError::Interrupted { .. } => "interrupted",
        LockError::Compromised(_) | LockError::Io(_) => "lock",
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
                EngineError::NoSuchDisplaced("x".into()),
                "no-such-displaced",
            ),
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
                    owner: Box::default(),
                },
                "session-owned",
            ),
            (
                EngineError::SessionOwned {
                    position: 1,
                    label: "a".into(),
                    owner: Box::new(SessionOwner {
                        damaged: vec![Damaged {
                            kind: DamagedKind::Record,
                            file: PathBuf::from("/p/sessions/7.json"),
                            detail: "not JSON".into(),
                        }],
                        ..SessionOwner::default()
                    }),
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
            (
                EngineError::LoginOverridden {
                    position: 1,
                    method: "api_key_helper".into(),
                    key_source: Some("apiKeyHelper".into()),
                },
                "login-overridden",
            ),
            (
                EngineError::LoginInvalid {
                    position: 1,
                    detail: "d".into(),
                },
                "login-invalid",
            ),
            (
                EngineError::LoginDrifted {
                    position: 1,
                    reported: "/x".into(),
                },
                "login-drifted",
            ),
            (
                EngineError::LoginUnknown {
                    position: 1,
                    detail: "d".into(),
                },
                "login-unknown",
            ),
            (
                EngineError::LaunchUnreachable { detail: "d".into() },
                "launch-unreachable",
            ),
            (
                EngineError::ReservationHeld { detail: "d".into() },
                "launch-unreachable",
            ),
            (
                EngineError::TargetChanged { why: "w".into() },
                "target-changed",
            ),
            (
                EngineError::Settings(SettingsError::UnknownKey("x".into())),
                "invalid-input",
            ),
            (
                EngineError::Settings(SettingsError::NotPerProvider("ui.color".into())),
                "invalid-input",
            ),
            (
                EngineError::Settings(SettingsError::ProviderMismatch {
                    prefix: "a".into(),
                    flag: "b".into(),
                }),
                "invalid-input",
            ),
            (
                EngineError::Settings(SettingsError::Invalid {
                    key: "k".into(),
                    reason: "must be x".into(),
                }),
                "invalid-input",
            ),
            (
                EngineError::Settings(SettingsError::UnknownProvider("p".into())),
                "invalid-input",
            ),
            (
                EngineError::Settings(SettingsError::Corrupt {
                    path: PathBuf::from("x"),
                    detail: "d".into(),
                }),
                "settings-unreadable",
            ),
            (
                EngineError::Settings(SettingsError::Lock(LockError::Timeout(PathBuf::from("x")))),
                "lock-timeout",
            ),
            (
                EngineError::Settings(SettingsError::Lock(LockError::Compromised(PathBuf::from(
                    "x",
                )))),
                "lock",
            ),
            (
                EngineError::Settings(SettingsError::Io(io::Error::other("x"))),
                "io",
            ),
            (
                EngineError::ReplacementUnreadable {
                    position: 1,
                    label: "a".into(),
                },
                "replacement-unreadable",
            ),
            (
                EngineError::RescueUnlistable {
                    path: PathBuf::from("r"),
                    detail: "d".into(),
                },
                "rescue-unreadable",
            ),
            (
                EngineError::EngineRunning {
                    provider: "p".into(),
                    pid: Some(7),
                },
                "engine-running",
            ),
            (
                EngineError::OrphanSessionRunning {
                    profile: PathBuf::from("s"),
                    owner: Box::default(),
                },
                "session-owned",
            ),
            (EngineError::PurgeChanged, "purge-changed"),
            (
                EngineError::AccountBroken {
                    position: 1,
                    reason: "r".into(),
                },
                "account-broken",
            ),
            (
                EngineError::Transfer(TransferError::NeedsPassphrase),
                "needs-passphrase",
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
    fn an_interrupted_settings_lock_wait_is_an_interruption() {
        // §14.1: the settings lock's wait is a cancellation point like any other, so its
        // interruption carries the signal the CLI exits with.
        let e = EngineError::Settings(SettingsError::Lock(LockError::Interrupted {
            path: PathBuf::from("locks/config.lock"),
            signal: 2,
        }));
        assert_eq!(e.signal(), Some(2));
        assert_eq!(e.kind(), "interrupted");
    }

    /// Decision 4: the settings lock fails with the kinds an engine lock fails with, for every
    /// way a lock can fail.
    #[test]
    fn the_settings_lock_has_the_engine_lock_s_kinds() {
        let failures = || {
            vec![
                LockError::Timeout(PathBuf::from("x")),
                LockError::Compromised(PathBuf::from("x")),
                LockError::Io(io::Error::other("x")),
                LockError::Interrupted {
                    path: PathBuf::from("x"),
                    signal: 2,
                },
            ]
        };
        for (engine, settings) in failures().into_iter().zip(failures()) {
            assert_eq!(
                EngineError::Settings(SettingsError::Lock(settings)).kind(),
                EngineError::Lock(engine).kind()
            );
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
