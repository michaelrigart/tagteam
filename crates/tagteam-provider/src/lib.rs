pub mod atomic;
pub mod cancel;
pub mod clock;
pub mod credential;
pub mod env;
pub mod flock;
pub mod http;
pub mod keychain;
pub mod liveness;
pub mod mkdir_lock;
#[cfg(feature = "mock-server")]
pub mod mock_server;
pub mod process;
pub mod profile;
pub mod provider;
pub mod read;
pub mod security;
pub mod splice;

/// Serialises the lib tests that fork a child or drop a flock and re-lock it: a child forked in
/// that window briefly holds a duplicate of the lock's open file description, so the re-lock
/// would spuriously see it held.
#[cfg(test)]
pub(crate) static FORK_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub use cancel::{Cancel, Interrupted};
pub use clock::{Clock, FakeClock, SystemClock};
pub use credential::{Credential, FreshCredential, Provenance};
pub use env::Env;
pub use flock::{FlockGuard, LockProbe, MutationGuard, probe_lock};
pub use http::{
    Http, HttpError, HttpRequest, HttpResponse, Method, NoHttp, RecordedRequest, ScriptedHttp,
};
#[cfg(feature = "file-keychain")]
pub use keychain::FileKeychain;
pub use keychain::{FakeKeychain, Keychain, KeychainError, LockState};
pub use liveness::{
    FakeProcess, FakeProcessProbe, ProcessProbe, RecordEntry, SessionRecord, SystemProcessProbe,
    parse_lstart, parse_session_record, read_session_records, record_is_live,
};
pub use mkdir_lock::{LockError, MkdirLock, MkdirLockSpec};
#[cfg(feature = "mock-server")]
pub use mock_server::{MockReply, MockRequest, MockServer};
pub use process::ProcessStamp;
pub use profile::{
    LAUNCH_DIR, LINKS_FILE, LinksRecord, MARKER_FILE, ProfileMarker, RunShell, SEED_FILE, Seed,
    canonical_profile_path, entry_matches, launch_reservations, profile_path,
};
pub use provider::{
    BeforeFallback, Capabilities, CapturedLogin, CredLocks, DeadReason, DoomedEntry, Identity,
    IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks, Provider,
    ProviderError, RefreshResult, SecretStore, StoredLogin, TransientKind, Undo, UsageResult,
    Written,
};
pub use read::{Read, ReadError};
pub use tagteam_core::pace::Pace;
pub use tagteam_core::poll::PollBudget;
pub use tagteam_core::usage::Window;
