use std::time::Duration;

use tagteam_core::AccountId;
use tagteam_provider::{Env, FlockGuard, LockError};

/// The per-account `flock` every vault writer holds (§6.2). The kernel releases it when the
/// holder exits; it never expires under a suspended holder.
#[derive(Debug)]
pub struct AccountLock {
    _guard: FlockGuard,
    id: AccountId,
}

impl AccountLock {
    pub const WAIT: Duration = Duration::from_secs(15);

    fn path(env: &Env, id: &AccountId) -> std::path::PathBuf {
        env.data_dir().join("locks").join(format!("{id}.lock"))
    }

    /// Waits up to `wait`, checking `env.cancel` before every attempt (§14.1).
    pub fn acquire(env: &Env, id: &AccountId, wait: Duration) -> Result<Self, LockError> {
        Ok(Self {
            _guard: FlockGuard::lock(&Self::path(env, id), wait, &env.cancel)?,
            id: id.clone(),
        })
    }

    pub fn try_acquire(env: &Env, id: &AccountId) -> Result<Option<Self>, LockError> {
        Ok(FlockGuard::try_lock(&Self::path(env, id))?.map(|g| Self {
            _guard: g,
            id: id.clone(),
        }))
    }

    pub fn id(&self) -> &AccountId {
        &self.id
    }
}
