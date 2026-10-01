pub mod atomic;
pub mod clock;
pub mod credential;
pub mod env;
pub mod flock;
pub mod http;
pub mod keychain;
pub mod mkdir_lock;
#[cfg(feature = "mock-server")]
pub mod mock_server;
pub mod process;
pub mod provider;
pub mod read;
pub mod security;
pub mod splice;

/// Serialises the lib tests that fork a child or drop a flock and re-lock it: a child forked in
/// that window briefly holds a duplicate of the lock's open file description, so the re-lock
/// would spuriously see it held.
#[cfg(test)]
pub(crate) static FORK_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub use clock::{Clock, FakeClock, SystemClock};
pub use credential::{Credential, FreshCredential, Provenance};
pub use env::Env;
pub use flock::{FlockGuard, MutationGuard};
pub use http::{
    Http, HttpError, HttpRequest, HttpResponse, Method, NoHttp, RecordedRequest, ScriptedHttp,
};
#[cfg(feature = "file-keychain")]
pub use keychain::FileKeychain;
pub use keychain::{FakeKeychain, Keychain, KeychainError, LockState};
pub use mkdir_lock::{LockError, MkdirLock, MkdirLockSpec};
#[cfg(feature = "mock-server")]
pub use mock_server::{MockReply, MockRequest, MockServer};
pub use process::ProcessStamp;
pub use provider::{
    BeforeFallback, Capabilities, CapturedLogin, CredLocks, DeadReason, DoomedEntry, Identity,
    IdentitySurface, KindTraits, LiveAuth, LiveChange, LiveLockSet, LiveLocks, Provider,
    ProviderError, RefreshResult, SecretStore, StoredLogin, TransientKind, Undo, UsageResult,
    Written,
};
pub use read::{Read, ReadError};
