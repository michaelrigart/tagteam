use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::{Clock, Env, Http, MutationGuard, Provider, Read};

use crate::account_lock::AccountLock;
use crate::error::EngineError;
use crate::hooks;
use crate::oracle::Oracle;
use crate::registry::ProviderRegistry;
use crate::settings::Settings;
use crate::store::Store;
use crate::vault::Vault;

pub struct EngineConfig {
    pub env: Env,
    pub registry: ProviderRegistry,
    pub vault: Vault,
    pub oracle: Arc<dyn Oracle>,
    pub clock: Arc<dyn Clock>,
    /// Every network request goes through this port (§4.4).
    pub http: Arc<dyn Http>,
    pub default_provider: ProviderId,
    /// `config.toml` as read for this command (§6.4).
    pub settings: Settings,
}

pub struct Engine {
    pub(crate) env: Env,
    pub(crate) registry: ProviderRegistry,
    pub(crate) vault: Vault,
    pub(crate) oracle: Arc<dyn Oracle>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) http: Arc<dyn Http>,
    pub(crate) default_provider: ProviderId,
    pub(crate) settings: Settings,
    store: Mutex<Option<Arc<Store>>>,
    #[cfg(feature = "test-hooks")]
    pub(crate) fail_at: Mutex<Option<&'static str>>,
    #[cfg(feature = "test-hooks")]
    #[allow(clippy::type_complexity)]
    pub(crate) on_point: Mutex<Option<(&'static str, Box<dyn Fn() + Send + Sync>)>>,
}

impl Engine {
    pub fn new(cfg: EngineConfig) -> Self {
        Self {
            env: cfg.env,
            registry: cfg.registry,
            vault: cfg.vault,
            oracle: cfg.oracle,
            clock: cfg.clock,
            http: cfg.http,
            default_provider: cfg.default_provider,
            settings: cfg.settings,
            store: Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            fail_at: Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            on_point: Mutex::new(None),
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

    /// The settings this engine was built with (§6.4).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The port providers send their requests through (§4.4).
    pub fn http(&self) -> &dyn Http {
        self.http.as_ref()
    }

    fn store_path(&self) -> PathBuf {
        self.env.data_dir().join("tagteam.db")
    }

    /// Opens the store, creating it (and its directory, 0700) when needed.
    pub fn store(&self) -> Result<Arc<Store>, EngineError> {
        let mut slot = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(s) = slot.as_ref() {
            return Ok(s.clone());
        }
        let s = Arc::new(Store::open(&self.store_path())?);
        *slot = Some(s.clone());
        Ok(s)
    }

    /// For read-only commands: never creates anything (§5).
    pub fn existing_store(&self) -> Result<Option<Arc<Store>>, EngineError> {
        let mut slot = self.store.lock().unwrap_or_else(PoisonError::into_inner);
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

    pub(crate) fn refuse_inside_run_shell(&self) -> Result<(), EngineError> {
        if self.env.inside_run_shell() {
            Err(EngineError::InsideRunShell)
        } else {
            Ok(())
        }
    }

    /// Whether an interrupted switch for the provider is on record (§9.6). Never creates the
    /// store.
    fn interrupted(&self, provider: &ProviderId) -> Result<bool, EngineError> {
        Ok(match self.existing_store()? {
            Some(s) => s.journal(provider)?.is_some(),
            None => false,
        })
    }

    /// The mutation lock for account-changing work, refused while an interrupted switch for
    /// the provider is still unresolved once recovery has run under it (§9.6). The refusal
    /// says why: a recovery that could not take the provider's live locks names the lock and
    /// asks for a retry; only a row recovery could not decide points at `--force`.
    pub(crate) fn guard_or_refuse(
        &self,
        provider: &ProviderId,
    ) -> Result<MutationGuard, EngineError> {
        let (guard, blocked) = self.guard_recovering(true)?;
        if self.interrupted(provider)? {
            return Err(blocked
                .into_iter()
                .find_map(|(p, e)| (&p == provider).then_some(e))
                .unwrap_or_else(|| EngineError::InterruptedSwitch(provider.to_string())));
        }
        Ok(guard)
    }

    /// Run before any planning or validation: if an interrupted switch is on record, take the
    /// mutation lock once (which recovers what it can, §9.6), then refuse if it is still
    /// unresolved. Creates nothing when there is no store.
    ///
    /// The final check runs while the guard from this same call is still held, and only then
    /// is the guard dropped: otherwise, between releasing it and re-checking, another process
    /// could take the guard and insert its own live journal row, which this call would then
    /// misreport as an interrupted switch (pointing the user at `--force` for a switch that is
    /// simply in progress elsewhere). That ordering can't be exercised deterministically by a
    /// test without a pause hook between acquiring the guard and running the check, which does
    /// not exist yet; it is verified by reading `guard_or_refuse` instead.
    pub(crate) fn settle_or_refuse(&self, provider: &ProviderId) -> Result<(), EngineError> {
        if self.interrupted(provider)? {
            drop(self.guard_or_refuse(provider)?);
        }
        Ok(())
    }

    /// tagteam's mutation lock. Before returning it, recovers every interrupted switch whose
    /// holder has died (§9.6). The oracle is asked before the lock is taken (§7.6).
    pub fn mutation_guard(&self) -> Result<MutationGuard, EngineError> {
        Ok(self.guard_recovering(true)?.0)
    }

    /// The mutation lock for commands that change only store metadata (`alias`, `disable`,
    /// `enable`, `move`): recovery still runs, but from fingerprints alone, so these commands
    /// never make a network call (§7.6).
    pub(crate) fn metadata_guard(&self) -> Result<MutationGuard, EngineError> {
        Ok(self.guard_recovering(false)?.0)
    }

    /// `mutation_guard`, with the refusal for each row whose recovery could not take its
    /// provider's live locks (`RecoveryBlocked`), by provider. With `ask_oracle` false the
    /// rows are recovered from fingerprints alone: no network call (§7.6, §9.6).
    fn guard_recovering(
        &self,
        ask_oracle: bool,
    ) -> Result<(MutationGuard, Vec<(ProviderId, EngineError)>), EngineError> {
        let hints: Vec<_> = self
            .dead_journals()?
            .into_iter()
            .map(|row| {
                let hint = if ask_oracle {
                    self.recovery_hints(&row)
                } else {
                    Vec::new()
                };
                (row, hint)
            })
            .collect();
        hooks::point(self, "before-mutation-lock")?;
        let guard = MutationGuard::acquire(&self.env, MutationGuard::TIMEOUT)?;
        // Enumerated again under the lock: a switch may have died while this command waited,
        // and its row is recovered now too, without a hint.
        let mut blocked = Vec::new();
        for row in self.dead_journals()? {
            let hint = hints
                .iter()
                .find(|(r, _)| *r == row)
                .map_or(&[][..], |(_, h)| h.as_slice());
            if let Err(e) = self.recover_one(&guard, &row, hint) {
                tracing::warn!(provider = %row.provider, "could not recover an interrupted switch: {e}");
                if matches!(e, EngineError::RecoveryBlocked { .. }) {
                    blocked.push((row.provider.clone(), e));
                }
            }
        }
        Ok((guard, blocked))
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

    pub(crate) fn reconcile_replacement(&self, lock: &AccountLock) -> Result<(), EngineError> {
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

#[cfg(feature = "test-hooks")]
impl Engine {
    pub fn fail_at(&self, name: Option<&'static str>) {
        *self.fail_at.lock().unwrap() = name;
    }

    /// Runs `callback` each time the switch passes the named point: a deterministic barrier
    /// for races that are otherwise timing-dependent.
    ///
    /// The callback runs with this engine's `on_point` mutex held, so it must never pass a
    /// hook point on this same engine: that would deadlock. Drive the race through a second
    /// engine instead (`Fx::engine_with_env`).
    pub fn on_point(&self, name: &'static str, callback: Box<dyn Fn() + Send + Sync>) {
        *self.on_point.lock().unwrap() = Some((name, callback));
    }
}

#[cfg(test)]
mod tests {
    use tagteam_core::AccountId;
    use tagteam_provider::{FakeKeychain, Identity, ProcessStamp};

    use super::*;
    use crate::oracle::NoOracle;
    use crate::store::{JournalRow, NewAccount};
    use crate::vault::KeychainVault;

    fn test_config(env: Env) -> EngineConfig {
        EngineConfig {
            env,
            registry: ProviderRegistry::new(),
            vault: Vault::new(Box::new(KeychainVault::new(Arc::new(FakeKeychain::new())))),
            oracle: Arc::new(NoOracle),
            clock: Arc::new(tagteam_provider::SystemClock),
            http: Arc::new(tagteam_provider::NoHttp),
            default_provider: ProviderId::new("p"),
            settings: Settings::default(),
        }
    }

    fn test_engine(env: Env) -> Engine {
        Engine::new(test_config(env))
    }

    #[test]
    fn the_engine_keeps_the_settings_it_was_built_with() {
        let d = tempfile::tempdir().unwrap();
        let settings = Settings {
            threshold: 75.0,
            models: vec!["Fable".into()],
            ..Settings::default()
        };
        let engine = Engine::new(EngineConfig {
            settings: settings.clone(),
            ..test_config(Env::for_test(d.path()))
        });
        assert_eq!(engine.settings(), &settings);
    }

    /// A journal row's `to_id` is foreign-keyed to `accounts`, so a test that installs one
    /// needs the account row it points at to already exist.
    fn seed_account(engine: &Engine, provider: &ProviderId, id: &AccountId) {
        engine
            .store()
            .unwrap()
            .insert_account(&NewAccount {
                id,
                provider,
                position: 1,
                identity_key: "a@b.co\n",
                identity: &Identity {
                    label: "a@b.co".into(),
                    email: Some("a@b.co".into()),
                    org_uuid: String::new(),
                    org_name: None,
                    account_uuid: None,
                    raw: serde_json::json!({}),
                },
                kind: "oauth",
                alias: None,
                login_expires_at: None,
                added_at: 1,
            })
            .unwrap();
    }

    /// A row recovery leaves alone (§9.6): its holder, this process, is still live. That keeps
    /// the refusal below about the row itself, not about whether recovery could decide it.
    fn unresolved_journal(provider: &ProviderId, to_id: &AccountId) -> JournalRow {
        JournalRow {
            provider: provider.clone(),
            holder: ProcessStamp::current().unwrap(),
            from_id: None,
            to_id: to_id.clone(),
            from_fp: None,
            from_identity: None,
            to_fp: "sha256:stale".into(),
            started_at: 1,
            prior: None,
        }
    }

    #[test]
    fn settle_or_refuse_only_errors_when_a_journal_row_is_on_record() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let engine = test_engine(env);
        let provider = ProviderId::new("p");

        assert!(engine.settle_or_refuse(&provider).is_ok());
        assert!(
            !engine.env().data_dir().exists(),
            "no journal on record: nothing is created (§5)"
        );

        let to_id = AccountId::from_string("acc");
        seed_account(&engine, &provider, &to_id);
        engine
            .store()
            .unwrap()
            .insert_journal(&unresolved_journal(&provider, &to_id))
            .unwrap();
        assert!(matches!(
            engine.settle_or_refuse(&provider),
            Err(EngineError::InterruptedSwitch(_))
        ));
    }
}
