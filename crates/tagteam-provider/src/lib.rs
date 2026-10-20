pub mod atomic;
pub mod cancel;
pub mod clock;
pub mod credential;
pub mod doctor;
pub mod env;
pub mod flock;
pub mod http;
pub mod keychain;
pub mod liveness;
pub mod mkdir_lock;
#[cfg(feature = "mock-server")]
pub mod mock_server;
pub mod passwd;
pub mod platform;
pub mod poison;
pub mod process;
pub mod profile;
pub mod provider;
pub mod read;
pub mod reservation;
pub mod security;
pub mod splice;
pub mod term;
pub mod transcript;

/// Serialises the lib tests that fork a child or drop a flock and re-lock it: a child forked in
/// that window briefly holds a duplicate of the lock's open file description, so the re-lock
/// would spuriously see it held.
///
/// Every test in this crate's lib tests that forks holds it (`fork_guard()`) for as long as its
/// children may be forked, however it starts them: `Command`, `spawn_session`, `spawn_foreground`,
/// `run_captured` and `ProcessRunner` all fork, and a program that cannot start still forks
/// before its exec fails. The reservation tests probe locks that a child forked by any other test
/// thread would hold between its fork and its exec, even though the descriptors are `O_CLOEXEC`.
#[cfg(test)]
pub(crate) static FORK_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Takes `FORK_GUARD`, ignoring poison: a test that panicked while holding it left nothing the
/// next one depends on.
#[cfg(test)]
pub(crate) fn fork_guard() -> std::sync::MutexGuard<'static, ()> {
    FORK_GUARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub use cancel::{Cancel, Interrupted};
#[cfg(any(test, feature = "test-doubles"))]
pub use clock::FakeClock;
pub use clock::{Clock, SystemClock};
pub use credential::{Credential, FreshCredential, Provenance};
pub use doctor::{Check, CheckStatus};
pub use env::{Env, EnvError};
pub use flock::{FlockGuard, LockProbe, MutationGuard, holders_of, probe_lock};
pub use http::{Http, HttpError, HttpRequest, HttpResponse, Method, NoHttp};
#[cfg(any(test, feature = "test-doubles"))]
pub use http::{RecordedRequest, ScriptedHttp};
#[cfg(any(test, feature = "test-doubles"))]
pub use keychain::FakeKeychain;
#[cfg(feature = "file-keychain")]
pub use keychain::FileKeychain;
pub use keychain::{Keychain, KeychainError, LockState, ReadCounts, ReadTally};
#[cfg(any(test, feature = "test-doubles"))]
pub use liveness::{FakeProcess, FakeProcessProbe};
pub use liveness::{
    ProcessProbe, RecordEntry, SessionRecord, SystemProcessProbe, parse_lstart,
    parse_session_record, read_session_records, read_session_records_dated, read_supervisor_lock,
    record_is_live,
};
pub use mkdir_lock::TimedOut;
pub use mkdir_lock::{LockError, MkdirLock, MkdirLockSpec};
#[cfg(feature = "mock-server")]
pub use mock_server::{MockReply, MockRequest, MockServer};
pub use platform::Platform;
#[cfg(any(test, feature = "test-doubles"))]
pub use process::ScriptedSpawner;
pub use process::{
    Captured, Discarded, ExitWatch, Liveness, ProcessSpawner, ProcessStamp, SpawnSpec,
    SystemSpawner, Watched, current_uid, exec_command, exit_code, find_on_path, run_bounded,
    run_captured, run_discarded, spawn_session,
};
pub use profile::{
    LAUNCH_DIR, LINKS_FILE, LinksRecord, MARKER_FILE, PENDING_LAUNCH_DIR, PendingEntry,
    PendingFile, PendingLaunch, ProfileMarker, RunShell, SEED_FILE, Seed, canonical_profile_path,
    entry_matches, has_own_file, launch_reservations, profile_path, read_own_bytes,
    refuse_linked_credential, refuse_linked_file, remove_own_file, write_own_json_with,
};
pub use provider::ValidateRequest;
pub use provider::{
    BeforeFallback, Capabilities, CapturedLogin, CredLocks, DeadReason, DoomedEntry, EntryKind,
    Identity, IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks,
    MergeReport, MustShare, Provider, ProviderError, RefreshResult, SecretStore, SessionEnv,
    SharePolicy, StoredLogin, TransientKind, Undo, UsageResult, Validity, Written,
};
pub use provider::{ProviderVars, ProviderWords};
pub use read::{Read, ReadError};
pub use reservation::{LaunchReservation, remove_dead_reservations};
pub use tagteam_core::pace::Pace;
pub use tagteam_core::poll::PollBudget;
pub use tagteam_core::usage::Window;
pub use transcript::{LineError, UsageLine};
