use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::liveness::ProcessProbe;
use tagteam_provider::process::ProcessSpawner;
use tagteam_provider::profile::RunShell;
use tagteam_provider::{Cancel, Clock, Env, Http, MutationGuard, Provider, ProviderError, Read};

use crate::account_lock::AccountLock;
use crate::error::EngineError;
use crate::hooks;
use crate::oracle::Oracle;
use crate::registry::ProviderRegistry;
use crate::settings::Settings;
use crate::store::{JournalRow, Store, StoreError};
use crate::vault::Vault;

/// How a holder of an account lock treats a pending replacement it cannot install (§12.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reconcile {
    /// Every holder but `remove` and `purge`: a replacement that landed with metadata that
    /// cannot be read refuses with `replacement-unreadable`.
    Strict,
    /// `remove` and `purge`, which delete the account either way (§12.5): such a replacement
    /// is left as it is, and the account goes with it.
    Removing,
}

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
    /// §4.2's process port, for session records (§12.6).
    pub process: Arc<dyn ProcessProbe>,
    /// The port the login check (`claude auth status`) spawns through (§12.3 step 8,
    /// Decision 10): `SystemSpawner` in production, `ScriptedSpawner` in tests.
    pub spawner: Arc<dyn ProcessSpawner>,
    /// §12.8, from `detect_run_shell`; `env` is already the effective (outer) environment.
    pub run_shell: RunShell,
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
    /// Judges session records (§12.6): `SystemProcessProbe` in production.
    pub(crate) process: Arc<dyn ProcessProbe>,
    pub(crate) spawner: Arc<dyn ProcessSpawner>,
    /// Where this process stands (§12.8).
    pub(crate) run_shell: RunShell,
    store: Mutex<Option<Arc<Store>>>,
    #[cfg(feature = "test-hooks")]
    pub(crate) fail_at: Mutex<Option<&'static str>>,
    #[cfg(feature = "test-hooks")]
    #[allow(clippy::type_complexity)]
    pub(crate) on_point: Mutex<Vec<(&'static str, Box<dyn Fn() + Send + Sync>)>>,
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
            process: cfg.process,
            spawner: cfg.spawner,
            run_shell: cfg.run_shell,
            store: Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            fail_at: Mutex::new(None),
            #[cfg(feature = "test-hooks")]
            on_point: Mutex::new(Vec::new()),
        }
    }

    pub fn env(&self) -> &Env {
        &self.env
    }

    /// The cancel token every cancellation point checks (§4.2, §14.1). It is the Env's, so
    /// every clone of that Env shares it; the CLI registers its signal handlers on it.
    pub fn cancel(&self) -> &Cancel {
        &self.env.cancel
    }

    /// A cancellation point outside the locks (§14.1): the interruption once the token holds a
    /// signal, so a request about to be sent is never sent.
    pub(crate) fn check_cancel(&self) -> Result<(), EngineError> {
        match self.cancel().requested() {
            Some(signal) => Err(EngineError::Interrupted(signal)),
            None => Ok(()),
        }
    }

    /// The Env for a write inside a critical span (§14.1): the same paths, with a cancel token
    /// nothing sets, so a lock wait the write makes (CC's storage-write lock, §9.1) runs to
    /// completion or times out. A signal stays recorded for the next cancellation point.
    pub(crate) fn critical_env(&self) -> Env {
        let mut env = self.env.clone();
        env.cancel = Cancel::new();
        env
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

    /// Where this process stands (§12.8): outside a run shell, inside one (`env` is then the
    /// outer home its marker records), or under a marker that cannot be read.
    pub fn run_shell(&self) -> &RunShell {
        &self.run_shell
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

    /// `id` as a log line's field, only when this build registers it (§14.2): an ID read from a
    /// marker or a store column is otherwise any text, an email among it.
    pub(crate) fn registered_id<'a>(
        &self,
        id: &'a ProviderId,
    ) -> Option<tracing::field::DisplayValue<&'a ProviderId>> {
        self.registry.get(id).map(|_| tracing::field::display(id))
    }

    /// §12.8: commands that change accounts or the live login refuse inside a run shell, and
    /// under a marker that cannot be read, which the refusal names.
    pub(crate) fn refuse_inside_run_shell(&self) -> Result<(), EngineError> {
        match &self.run_shell {
            RunShell::Inside { .. } => Err(EngineError::InsideRunShell),
            RunShell::Outside | RunShell::Unreadable { .. } => self.refuse_unreadable_run_shell(),
        }
    }

    /// §12.8: under a marker that cannot be read the outer home is unknown, so every command
    /// but `statusline` refuses, naming it. Inside a readable run shell, `env` is already the
    /// outer home (Decision 6), so work that §12.8 lets see the default home goes on.
    pub(crate) fn refuse_unreadable_run_shell(&self) -> Result<(), EngineError> {
        match &self.run_shell {
            RunShell::Outside | RunShell::Inside { .. } => Ok(()),
            RunShell::Unreadable { marker, detail } => Err(EngineError::RunShellUnreadable {
                marker: marker.clone(),
                detail: detail.clone(),
            }),
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
        self.guard_or_refuse_as(provider, "cli")
    }

    /// `guard_or_refuse`, recording each switch it recovers with `source` (§9.4 step 9: `cli`,
    /// or `auto` for an auto-switch engine, §11.2 step 2).
    pub(crate) fn guard_or_refuse_as(
        &self,
        provider: &ProviderId,
        source: &'static str,
    ) -> Result<MutationGuard, EngineError> {
        self.guard_or_refuse_for(provider, source, MutationGuard::TIMEOUT)
    }

    /// `guard_or_refuse`, waiting up to `timeout` for the lock. `run`'s launch waits 30 s
    /// (§9.1), because another launch may hold it through a bootstrap's validation.
    pub(crate) fn guard_or_refuse_within(
        &self,
        provider: &ProviderId,
        timeout: Duration,
    ) -> Result<MutationGuard, EngineError> {
        self.guard_or_refuse_for(provider, "cli", timeout)
    }

    /// `guard_or_refuse_as`, waiting up to `timeout` for the lock.
    fn guard_or_refuse_for(
        &self,
        provider: &ProviderId,
        source: &'static str,
        timeout: Duration,
    ) -> Result<MutationGuard, EngineError> {
        self.guard_or_refuse_each_within(std::slice::from_ref(provider), source, timeout)
    }

    /// `guard_or_refuse_as` for several providers under one mutation lock (`import`, §13.3):
    /// refused for the first of `providers` whose interrupted switch is still unresolved once
    /// recovery has run under it.
    pub(crate) fn guard_or_refuse_each(
        &self,
        providers: &[ProviderId],
        source: &'static str,
    ) -> Result<MutationGuard, EngineError> {
        self.guard_or_refuse_each_within(providers, source, MutationGuard::TIMEOUT)
    }

    /// `guard_or_refuse_each`, waiting up to `timeout` for the lock.
    fn guard_or_refuse_each_within(
        &self,
        providers: &[ProviderId],
        source: &'static str,
        timeout: Duration,
    ) -> Result<MutationGuard, EngineError> {
        let (guard, mut blocked) = self.guard_recovering(true, source, timeout)?;
        for provider in providers {
            if self.interrupted(provider)? {
                return Err(match blocked.iter().position(|(p, _)| p == provider) {
                    Some(i) => blocked.swap_remove(i).1,
                    None => EngineError::InterruptedSwitch(provider.to_string()),
                });
            }
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
        self.settle_or_refuse_as(provider, "cli")
    }

    /// `settle_or_refuse`, recording each switch it recovers with `source`.
    pub(crate) fn settle_or_refuse_as(
        &self,
        provider: &ProviderId,
        source: &'static str,
    ) -> Result<(), EngineError> {
        if self.interrupted(provider)? {
            drop(self.guard_or_refuse_as(provider, source)?);
        }
        Ok(())
    }

    /// tagteam's mutation lock. Before returning it, recovers every interrupted switch whose
    /// holder has died (§9.6). The oracle is asked before the lock is taken (§7.6).
    pub fn mutation_guard(&self) -> Result<MutationGuard, EngineError> {
        Ok(self
            .guard_recovering(true, "cli", MutationGuard::TIMEOUT)?
            .0)
    }

    /// The mutation lock for commands that change only store metadata (`alias`, `disable`,
    /// `enable`, `move`): recovery still runs, but from fingerprints alone, so these commands
    /// never make a network call (§7.6).
    pub(crate) fn metadata_guard(&self) -> Result<MutationGuard, EngineError> {
        Ok(self
            .guard_recovering(false, "cli", MutationGuard::TIMEOUT)?
            .0)
    }

    /// `mutation_guard`, with the refusal for each row whose recovery could not take its
    /// provider's live locks or whose entry its agent wrote meanwhile (`RecoveryBlocked`,
    /// `RecoveryMoved`), by provider. With `ask_oracle` false the
    /// rows are recovered from fingerprints alone: no network call (§7.6, §9.6). A recovery
    /// interrupted at one of its lock waits ends the command with that interruption (§14.1).
    /// Each recovered switch is recorded with `source`. The lock is waited for up to `timeout`.
    fn guard_recovering(
        &self,
        ask_oracle: bool,
        source: &'static str,
        timeout: Duration,
    ) -> Result<(MutationGuard, Vec<(ProviderId, EngineError)>), EngineError> {
        self.guard_recovering_from(ask_oracle, source, timeout, Self::dead_journals)
    }

    /// `MutationGuard`, taken after §9.1's pre-wait: each provider's config lock is waited for,
    /// holding no lock of tagteam's or of CC's, until it is absent or stale, so neither
    /// `MutationGuard` nor the credential locks CC's refresh waits on are held through the
    /// wait. Every acquisition of the guard goes through here. A lock a process holds for its
    /// whole lifetime (an `auto` engine's, §11.1) stays held. A timeout refuses as a lock
    /// timeout does.
    pub(crate) fn acquire_guard(&self, timeout: Duration) -> Result<MutationGuard, EngineError> {
        for p in self.registry.all() {
            // A lock failure is the guard's own kind of failure (`EngineError::Lock`), which
            // callers that treat a busy guard as "moved" or "no switch" already match.
            p.wait_config_lock_idle(&self.env).map_err(|e| match e {
                ProviderError::Lock(e) => EngineError::Lock(e),
                e => EngineError::Provider(e),
            })?;
        }
        hooks::point(self, "config-pre-wait-done")?;
        Ok(MutationGuard::acquire(&self.env, timeout)?)
    }

    /// `guard_recovering` over the rows `journals` gives, read before the lock and again under
    /// it. `purge_guard` passes `dead_decodable_journals`, so a row that does not decode is
    /// left for it to delete (§10.5 step 5) rather than ending it; every other caller reads
    /// them all, strictly.
    fn guard_recovering_from(
        &self,
        ask_oracle: bool,
        source: &'static str,
        timeout: Duration,
        journals: fn(&Self) -> Result<Vec<JournalRow>, EngineError>,
    ) -> Result<(MutationGuard, Vec<(ProviderId, EngineError)>), EngineError> {
        let hints: Vec<_> = journals(self)?
            .into_iter()
            .map(|row| {
                let hint = if ask_oracle {
                    self.recovery_hints(&row)?
                } else {
                    Vec::new()
                };
                Ok((row, hint))
            })
            .collect::<Result<_, EngineError>>()?;
        hooks::point(self, "before-mutation-lock")?;
        let guard = self.acquire_guard(timeout)?;
        // Enumerated again under the lock: a switch may have died while this command waited,
        // and its row is recovered now too, without a hint.
        let mut blocked = Vec::new();
        for row in journals(self)? {
            let hint = hints
                .iter()
                .find(|(r, _)| *r == row)
                .map_or(&[][..], |(_, h)| h.as_slice());
            if let Err(e) = self.recover_one(&guard, &row, hint, source) {
                // CC wrote the entry recovery was clearing: nothing was cleared, the row stays,
                // and a plain retry settles it.
                let e = match e {
                    EngineError::Provider(ProviderError::EntryMoved(_)) => {
                        match self.provider(&row.provider) {
                            Ok(p) => EngineError::RecoveryMoved {
                                provider: row.provider.to_string(),
                                app: p.display_name(),
                            },
                            Err(e) => e,
                        }
                    }
                    e => e,
                };
                // Interrupted at a lock wait, the recovery wrote nothing and its row stays for
                // the next command. Reported as itself, never as a switch it could not settle.
                if e.signal().is_some() {
                    return Err(e);
                }
                // By kind (§14.2): the error may name an account's label, a path, or a provider
                // as the row stored it.
                tracing::warn!(
                    provider = self.registered_id(&row.provider),
                    kind = e.kind(),
                    "could not recover an interrupted switch"
                );
                if matches!(
                    e,
                    EngineError::RecoveryBlocked { .. } | EngineError::RecoveryMoved { .. }
                ) {
                    blocked.push((row.provider.clone(), e));
                }
            }
        }
        Ok((guard, blocked))
    }

    /// §10.5 steps 4 and 5 (Decision 7): `MutationGuard`, under which every interrupted switch
    /// whose holder died is recovered as usual (§9.6). What recovery could not settle is left
    /// for `purge_leftover_journals`, which runs once step 6's refusals have passed: a refused
    /// purge deletes nothing, so the next guarded command still finds the interrupted switch.
    pub(crate) fn purge_guard(&self) -> Result<MutationGuard, EngineError> {
        let (guard, _blocked) = self.guard_recovering_from(
            true,
            "cli",
            MutationGuard::TIMEOUT,
            Self::dead_decodable_journals,
        )?;
        Ok(guard)
    }

    /// §10.5 step 5 (Decision 7), after step 6: a journal row of an affected provider that is
    /// still there once recovery is done (undecidable, blocked, or unreadable) is deleted, with
    /// a warning that the live login may be incoherent, and the live login is left as it is:
    /// purge is the way out of a state tagteam cannot repair, so it never refuses on one.
    /// Returns the warnings. The caller holds the guard `purge_guard` gave.
    pub(crate) fn purge_leftover_journals(
        &self,
        providers: &[ProviderId],
    ) -> Result<Vec<String>, EngineError> {
        let mut warnings = Vec::new();
        if let Some(store) = self.existing_store()? {
            for provider in providers {
                if matches!(store.journal(provider), Ok(None)) {
                    continue;
                }
                store.delete_journal(provider)?;
                tracing::warn!(
                    provider = self.registered_id(provider),
                    "purge deleted an interrupted switch's record that recovery could not settle"
                );
                warnings.push(format!(
                    "an interrupted switch for {provider} could not be recovered, so its record was deleted; the live login may be incoherent (its credential and its identity may name different accounts), and it is left as it is"
                ));
            }
        }
        Ok(warnings)
    }

    /// `dead_journals` for `purge_guard`: each row decoded on its own, and one that does not
    /// decode left out of recovery. It is one recovery cannot decide, so `purge_guard` deletes
    /// it with the rest (§10.5 step 5, Decision 7).
    fn dead_decodable_journals(&self) -> Result<Vec<JournalRow>, EngineError> {
        let Some(store) = self.existing_store()? else {
            return Ok(vec![]);
        };
        Ok(store
            .journals_each()?
            .into_iter()
            .filter_map(Result::ok)
            .filter(|j| !j.holder.is_live())
            .collect())
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

    /// §12.5's reconciliation, strictly: every lock holder but `remove` and `purge` runs it.
    pub(crate) fn reconcile_replacement(&self, lock: &AccountLock) -> Result<(), EngineError> {
        self.reconcile_replacement_as(lock, Reconcile::Strict)
    }

    /// §12.5: a pending replacement the vault holds (`replacing_fp`) landed, and its recorded
    /// metadata is installed; one it does not hold never landed, and is rolled back, which needs
    /// no metadata. A landed one whose metadata cannot be read refuses under `Strict`, naming
    /// the account, and is left as it is under `Removing`.
    pub(crate) fn reconcile_replacement_as(
        &self,
        lock: &AccountLock,
        mode: Reconcile,
    ) -> Result<(), EngineError> {
        let Some(store) = self.existing_store()? else {
            return Ok(());
        };
        let Some(row) = store.account(lock.id())? else {
            return Ok(());
        };
        let Some(fp) = row.replacing_fp.as_deref() else {
            return Ok(());
        };
        let provider = self.provider(&row.provider)?;
        match self.vault.read(lock.id()) {
            Read::Present(b) if provider.fingerprint(&b).is_some_and(|f| f.as_str() == fp) => {
                match store.finish_replacement(lock.id(), self.now_ms()) {
                    Ok(()) => {}
                    Err(StoreError::ReplacementUnreadable(_)) if mode == Reconcile::Removing => {}
                    Err(StoreError::ReplacementUnreadable(detail)) => {
                        tracing::warn!(
                            position = row.position,
                            account = %row.id,
                            "a new login landed but what its replacement recorded cannot be read ({detail})"
                        );
                        return Err(EngineError::ReplacementUnreadable {
                            position: row.position,
                            label: row.label.clone(),
                        });
                    }
                    Err(e) => return Err(e.into()),
                }
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
        *self.on_point.lock().unwrap() = vec![(name, callback)];
    }

    /// `on_point` for one more point, keeping what is registered: a test that must act at two
    /// points of one run (a lock taken at one and released at another).
    pub fn also_on_point(&self, name: &'static str, callback: Box<dyn Fn() + Send + Sync>) {
        self.on_point.lock().unwrap().push((name, callback));
    }
}

#[cfg(test)]
mod tests {
    use tagteam_core::AccountId;
    use tagteam_provider::profile::ProfileMarker;
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
            process: Arc::new(tagteam_provider::liveness::FakeProcessProbe::new()),
            spawner: Arc::new(tagteam_provider::process::ScriptedSpawner::new()),
            run_shell: RunShell::Outside,
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
            to_epoch: None,
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

    #[test]
    fn refuse_inside_run_shell_follows_the_three_states() {
        let d = tempfile::tempdir().unwrap();
        let with = |run_shell: RunShell| {
            Engine::new(EngineConfig {
                run_shell,
                ..test_config(Env::for_test(d.path()))
            })
        };
        assert!(with(RunShell::Outside).refuse_inside_run_shell().is_ok());
        assert!(
            with(RunShell::Outside)
                .refuse_unreadable_run_shell()
                .is_ok()
        );
        let inside = RunShell::Inside {
            profile: PathBuf::from("/p"),
            marker: ProfileMarker {
                provider: ProviderId::new("p"),
                account_id: AccountId::from_string("a"),
                config_dir: "/p".into(),
                outer: serde_json::json!({}),
            },
        };
        let engine = with(inside.clone());
        assert_eq!(engine.run_shell(), &inside);
        assert!(matches!(
            engine.refuse_inside_run_shell(),
            Err(EngineError::InsideRunShell)
        ));
        assert!(
            engine.refuse_unreadable_run_shell().is_ok(),
            "a readable run shell's outer home is known"
        );
        let unreadable = with(RunShell::Unreadable {
            marker: PathBuf::from("/p/.tagteam-profile.json"),
            detail: "not JSON".into(),
        });
        for err in [
            unreadable.refuse_inside_run_shell().unwrap_err(),
            unreadable.refuse_unreadable_run_shell().unwrap_err(),
        ] {
            assert_eq!(err.kind(), "run-shell-unreadable");
            assert_eq!(
                err.to_string(),
                "the run-shell marker /p/.tagteam-profile.json cannot be read (not JSON)"
            );
        }
    }
}
