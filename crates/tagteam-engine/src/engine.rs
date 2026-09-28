use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::{Clock, Env, MutationGuard, Provider, Read};

use crate::account_lock::AccountLock;
use crate::error::EngineError;
use crate::oracle::Oracle;
use crate::registry::ProviderRegistry;
use crate::store::Store;
use crate::vault::Vault;

pub struct EngineConfig {
    pub env: Env,
    pub registry: ProviderRegistry,
    pub vault: Vault,
    pub oracle: Arc<dyn Oracle>,
    pub clock: Arc<dyn Clock>,
    pub default_provider: ProviderId,
}

pub struct Engine {
    pub(crate) env: Env,
    pub(crate) registry: ProviderRegistry,
    pub(crate) vault: Vault,
    #[expect(dead_code, reason = "read by the switch, Task 20")]
    pub(crate) oracle: Arc<dyn Oracle>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) default_provider: ProviderId,
    store: Mutex<Option<Arc<Store>>>,
}

impl Engine {
    pub fn new(cfg: EngineConfig) -> Self {
        Self {
            env: cfg.env,
            registry: cfg.registry,
            vault: cfg.vault,
            oracle: cfg.oracle,
            clock: cfg.clock,
            default_provider: cfg.default_provider,
            store: Mutex::new(None),
        }
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    pub fn default_provider(&self) -> &ProviderId {
        &self.default_provider
    }

    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    fn store_path(&self) -> PathBuf {
        self.env.data_dir().join("tagteam.db")
    }

    /// Opens the store, creating it (and its directory, 0700) when needed.
    pub fn store(&self) -> Result<Arc<Store>, EngineError> {
        let mut slot = self.store.lock().unwrap();
        if let Some(s) = slot.as_ref() {
            return Ok(s.clone());
        }
        let s = Arc::new(Store::open(&self.store_path())?);
        *slot = Some(s.clone());
        Ok(s)
    }

    /// For read-only commands: never creates anything (§5).
    pub fn existing_store(&self) -> Result<Option<Arc<Store>>, EngineError> {
        let mut slot = self.store.lock().unwrap();
        if let Some(s) = slot.as_ref() {
            return Ok(Some(s.clone()));
        }
        match Store::open_existing(&self.store_path())? {
            Some(s) => {
                let s = Arc::new(s);
                *slot = Some(s.clone());
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    pub fn provider(&self, id: &ProviderId) -> Result<Arc<dyn Provider>, EngineError> {
        self.registry
            .get(id)
            .ok_or_else(|| EngineError::UnknownProvider(id.to_string()))
    }

    pub fn providers(&self) -> Vec<Arc<dyn Provider>> {
        self.registry.all().to_vec()
    }

    #[expect(dead_code, reason = "used by later commands, Task 18")]
    pub(crate) fn refuse_inside_run_shell(&self) -> Result<(), EngineError> {
        if self.env.inside_run_shell() {
            Err(EngineError::InsideRunShell)
        } else {
            Ok(())
        }
    }

    /// Account-changing work refuses while an interrupted switch for the provider is
    /// unresolved (§9.6). Never creates the store.
    pub(crate) fn refuse_if_interrupted(&self, provider: &ProviderId) -> Result<(), EngineError> {
        match self.existing_store()? {
            Some(s) if s.journal(provider)?.is_some() => {
                Err(EngineError::InterruptedSwitch(provider.to_string()))
            }
            _ => Ok(()),
        }
    }

    /// Run before any planning or validation: if an interrupted switch is on record, take the
    /// mutation lock once (which recovers what it can, Task 21), then refuse if it is still
    /// unresolved. Creates nothing when there is no store.
    #[expect(dead_code, reason = "used by later commands, Task 18 and Task 20")]
    pub(crate) fn settle_or_refuse(&self, provider: &ProviderId) -> Result<(), EngineError> {
        let pending = match self.existing_store()? {
            Some(s) => s.journal(provider)?.is_some(),
            None => false,
        };
        if pending {
            drop(self.mutation_guard()?);
            self.refuse_if_interrupted(provider)?;
        }
        Ok(())
    }

    /// tagteam's mutation lock. Task 21 adds interrupted-switch recovery here.
    pub fn mutation_guard(&self) -> Result<MutationGuard, EngineError> {
        Ok(MutationGuard::acquire(&self.env, MutationGuard::TIMEOUT)?)
    }

    /// Takes the account lock, then reconciles a pending explicit replacement (§12.5): the
    /// replacer held this lock throughout, so finding its marker means it died.
    pub fn lock_account(&self, id: &AccountId) -> Result<AccountLock, EngineError> {
        let lock = AccountLock::acquire(&self.env, id, AccountLock::WAIT)?;
        self.reconcile_replacement(&lock)?;
        Ok(lock)
    }

    pub fn lock_accounts(&self, ids: &[&AccountId]) -> Result<Vec<AccountLock>, EngineError> {
        let mut sorted: Vec<&AccountId> = ids.to_vec();
        sorted.sort();
        sorted.dedup();
        sorted.into_iter().map(|id| self.lock_account(id)).collect()
    }

    fn reconcile_replacement(&self, lock: &AccountLock) -> Result<(), EngineError> {
        let Some(store) = self.existing_store()? else {
            return Ok(());
        };
        let Some(row) = store.account(lock.id())? else {
            return Ok(());
        };
        let Some(fp) = row.replacing_fp else {
            return Ok(());
        };
        let provider = self.provider(&row.provider)?;
        match self.vault.read(lock.id()) {
            Read::Present(b) if provider.fingerprint(&b).is_some_and(|f| f.as_str() == fp) => {
                store.finish_replacement(lock.id())?
            }
            Read::Present(_) | Read::Absent => store.rollback_replacement(lock.id())?,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        }
        Ok(())
    }
}
