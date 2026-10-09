//! §10.5: `tagteam purge` deletes tagteam's data: every account of a provider, or with no
//! `--provider`, everything tagteam stores. Each account goes as `remove` deletes one (§10.3),
//! vault first, so a purge that stops part-way is finished by running it again (B.66). It never
//! deletes or replaces a provider's live login. The CLI asks between `purge_plan`, which takes no
//! lock and creates nothing, and `purge`, which refuses when the accounts it would delete are
//! not the ones the plan named (Decision 6).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::atomic::temp_writer_pid;
use tagteam_provider::env::LOG_ROTATIONS;
use tagteam_provider::profile::{ProfileMarker, canonical_profile_path, profile_path};
use tagteam_provider::{FlockGuard, Provider, Read};

use crate::account_lock::AccountLock;
use crate::auto::{engine_lock_path, read_holder};
use crate::displace::PurgeError;
use crate::engine::Engine;
use crate::error::{EngineError, SessionOwner};
use crate::hooks;
use crate::lifecycle::{UnlistedRescues, absent, identity, trace_path};
use crate::rescue::RescueUnlisted;
use crate::session::{SessionState, session_owned_error};
use crate::store::{AccountRow, Store};
use crate::vault::Leftovers;

/// §10.5: Keychain items are not a provider's, so the flag that deletes the ones no account
/// names cannot be narrowed to one.
pub const KEYCHAIN_ORPHANS_WITH_PROVIDER: &str = "--keychain-orphans deletes Keychain items that no account names, and so no provider; run it without --provider";

/// §10.5: the warning a full purge without `--keychain-orphans` gives when `tagteam` items
/// remain once its accounts are gone.
pub const KEYCHAIN_LEFTOVERS: &str = "the Keychain still holds `tagteam` items that no account of this store names; they may belong to another tagteam data directory on this Mac, and `tagteam purge --keychain-orphans` deletes them, for every data directory";

/// What a purge will delete, for the summary the CLI confirms (§10.5 step 2). `purge_plan`
/// builds it without a lock, so `purge` checks its accounts again under the guard.
#[derive(Debug, Clone, PartialEq)]
pub struct PurgePlan {
    /// `--provider`'s; `None` for a full purge.
    pub provider: Option<ProviderId>,
    /// The accounts it deletes, by provider, then position.
    pub accounts: Vec<PurgeAccount>,
    /// Session profiles no store account owns that it deletes (§10.5 step 6).
    pub orphan_profiles: Vec<PathBuf>,
    /// Rescue files (refreshed tokens not yet in the vault) it deletes. In a full purge, a
    /// `rescue` path that is not a directory counts as one.
    pub rescues: usize,
    /// Displaced credentials it deletes.
    pub displaced: usize,
    /// A full purge also empties the store and deletes the log.
    pub store_and_log: bool,
    /// `--keychain-orphans`: a full purge also deletes every `tagteam` Keychain item no account
    /// of this store names, which may be another data directory's. `purge_plan` leaves it off;
    /// the CLI sets it from the flag.
    pub keychain_orphans: bool,
}

/// One account a purge deletes.
#[derive(Debug, Clone, PartialEq)]
pub struct PurgeAccount {
    pub id: AccountId,
    pub provider: ProviderId,
    pub position: u32,
    pub label: String,
    /// It has a session profile, which goes with it (§10.3).
    pub has_profile: bool,
}

/// What a purge deleted, and what it could not (§10.5 "Result").
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PurgeReport {
    /// The accounts deleted whole.
    pub accounts: Vec<PurgeAccount>,
    pub displaced: usize,
    pub rescues: usize,
    /// A full purge emptied the store (or found none).
    pub store_emptied: bool,
    /// What the user should know, deletion went on regardless.
    pub warnings: Vec<String>,
    /// `(what, message)` for each thing it could not delete. Any makes the command exit 1.
    pub failures: Vec<(String, String)>,
}

/// The accounts a purge of `provider` (or of everything) affects, by provider, then position.
fn affected_rows(
    store: Option<&Store>,
    provider: Option<&ProviderId>,
) -> Result<Vec<AccountRow>, EngineError> {
    Ok(match (store, provider) {
        (None, _) => Vec::new(),
        (Some(s), Some(p)) => s.accounts(p)?,
        (Some(s), None) => s.all_accounts()?,
    })
}

/// Every stored account's ID, whatever its provider: a `sessions/` entry named by one is that
/// account's profile, never an orphan.
fn owned_ids(store: Option<&Store>) -> Result<BTreeSet<String>, EngineError> {
    Ok(match store {
        None => BTreeSet::new(),
        Some(s) => s
            .all_accounts()?
            .into_iter()
            .map(|r| r.id.as_str().to_owned())
            .collect(),
    })
}

/// How many entries `path` holds, for the summary: a directory's entries (none if it cannot
/// be listed), 1 for anything else there, 0 when nothing is.
fn entries_in(path: &Path) -> usize {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::read_dir(path).map_or(0, Iterator::count),
        Ok(_) => 1,
        Err(_) => 0,
    }
}

/// Deletes `path` whatever it is, a link as a link; absent is done. Returns how many entries
/// it held: a directory's, else 1.
fn remove_path(path: &Path) -> io::Result<usize> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    if meta.is_dir() {
        let held = fs::read_dir(path)?.count();
        fs::remove_dir_all(path)?;
        Ok(held)
    } else {
        fs::remove_file(path)?;
        Ok(1)
    }
}

/// The failure a purge reports for store rows it kept: `unfinished` accounts could not be
/// deleted, and their rows are what `again` finishes them from (§10.5, B.66).
fn store_kept(unfinished: usize, also: &str, again: &str) -> String {
    let (accounts, them) = if unfinished == 1 {
        ("1 account".to_owned(), "it")
    } else {
        (format!("{unfinished} accounts"), "them")
    };
    format!("kept{also}, since {accounts} could not be deleted: `{again}` finishes {them}")
}

fn account_of(row: &AccountRow, has_profile: bool) -> PurgeAccount {
    PurgeAccount {
        id: row.id.clone(),
        provider: row.provider.clone(),
        position: row.position,
        label: row.label.clone(),
        has_profile,
    }
}

/// The Keychain items an orphan is given: each with the provider that names it.
type OrphanItems = Vec<(Arc<dyn Provider>, String)>;

impl Engine {
    /// §10.5 step 2's summary. Inside a run shell it refuses at once (§12.8). It reads the
    /// store and the data directory only, takes no lock and creates nothing (§5). An account
    /// of a provider this build does not register is listed with its provider's ID.
    pub fn purge_plan(&self, provider: Option<&ProviderId>) -> Result<PurgePlan, EngineError> {
        self.refuse_inside_run_shell()?;
        let store = self.existing_store()?;
        if let Some(p) = provider {
            self.purgeable(store.as_deref(), p)?;
        }
        let rows = affected_rows(store.as_deref(), provider)?;
        let orphan_profiles = self.orphan_profiles(&owned_ids(store.as_deref())?, provider)?;
        let (rescues, displaced) = match provider {
            Some(p) => (
                rows.iter()
                    .map(|r| self.rescue_paths_for(&r.id).map_or(0, |paths| paths.len()))
                    .sum(),
                self.displaced_of(p)?.len(),
            ),
            None => (
                entries_in(&self.rescue_dir()),
                entries_in(&self.env.data_dir().join("displaced")),
            ),
        };
        let accounts = rows
            .iter()
            .map(|r| {
                let has_profile = fs::symlink_metadata(profile_path(&self.env, &r.id)).is_ok();
                account_of(r, has_profile)
            })
            .collect();
        Ok(PurgePlan {
            provider: provider.cloned(),
            accounts,
            orphan_profiles,
            rescues,
            displaced,
            store_and_log: provider.is_none(),
            keychain_orphans: false,
        })
    }

    /// §10.5 steps 3 to 9, for a `plan` the user confirmed. Every refusal comes before anything
    /// is deleted. Then each account goes under its account lock, then the orphaned profiles,
    /// then the provider's other rows, or with no `--provider` everything else: the vault's
    /// leftovers, the `rescue` path, `displaced/`, dead temp files, the store's rows and, last,
    /// the log. Something that cannot be deleted is reported and the purge goes on; an
    /// interruption stops it at the next account, and running it again finishes it.
    pub fn purge(&self, plan: &PurgePlan) -> Result<PurgeReport, EngineError> {
        self.refuse_inside_run_shell()?;
        if plan.keychain_orphans && plan.provider.is_some() {
            return Err(EngineError::InvalidInput(
                KEYCHAIN_ORPHANS_WITH_PROVIDER.into(),
            ));
        }
        let providers: Vec<ProviderId> = match &plan.provider {
            Some(p) => {
                self.purgeable(self.existing_store()?.as_deref(), p)?;
                vec![p.clone()]
            }
            // Every registered provider, and each one this build does not register that the
            // plan names an account of: its engine lock and its interrupted switch are this
            // purge's too. An account of another one added since is `purge-changed` below.
            None => {
                let mut all: Vec<ProviderId> = self.registry.all().iter().map(|p| p.id()).collect();
                for a in &plan.accounts {
                    if !all.contains(&a.provider) {
                        all.push(a.provider.clone());
                    }
                }
                all
            }
        };
        if !self.env.data_dir().try_exists()? {
            // §5: with no data directory there is no account, profile, rescue, displaced
            // entry or store to delete, and nothing to protect, so no lock file is created for
            // one. The Keychain's leftovers and the log are elsewhere.
            if !plan.accounts.is_empty() {
                return Err(EngineError::PurgeChanged);
            }
            if let Some(report) = self.purge_without_data(plan)? {
                return Ok(report);
            }
            // A data directory appeared meanwhile (an `add`): the guarded path below takes it,
            // and step 6 refuses the accounts the confirmed plan did not name.
        }
        // 3. No auto-switch engine may start while the purge runs.
        let _engines = self.hold_engine_locks(&providers)?;
        // 4 and 5, held to the end: nothing is created behind the purge.
        let _guard = self.purge_guard()?;
        hooks::point(self, "purge-guarded")?;
        // 6.
        let store = self.existing_store()?;
        let rows = affected_rows(store.as_deref(), plan.provider.as_ref())?;
        let now: BTreeSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        let confirmed: BTreeSet<&str> = plan.accounts.iter().map(|a| a.id.as_str()).collect();
        if now != confirmed {
            return Err(EngineError::PurgeChanged);
        }
        for row in &rows {
            match self.provider(&row.provider) {
                // §10.3 Guard, as `remove` has it: a session-owned account, or a profile whose
                // history deleting it would delete or leave split.
                Ok(p) => {
                    self.refuse_session_owned(p.as_ref(), row)?;
                    self.refuse_profile_split(p.as_ref(), row)?;
                    self.refuse_live_profile_item(p.as_ref(), row)?;
                }
                // Its provider's share lists are unknown: every registered provider's judge.
                Err(_) => {
                    self.refuse_unregistered_in_use(row, &providers)?;
                    self.refuse_real_copies(&profile_path(&self.env, &row.id))?;
                    self.refuse_live_files_in_unregistered(row, &providers)?;
                }
            }
        }
        let orphans =
            self.orphan_profiles(&owned_ids(store.as_deref())?, plan.provider.as_ref())?;
        for profile in &orphans {
            self.refuse_orphan_in_use(profile, &providers)?;
            self.refuse_orphan_split(profile)?;
            // §10.5: the live login is found here, before anything is deleted, not when this
            // entry's turn comes. The user is told which entry.
            self.refuse_orphan_live(profile, &providers).map_err(|e| {
                EngineError::Io(io::Error::other(format!("{}: {e}", profile.display())))
            })?;
        }
        // §10.5: nothing else purge deletes may hold the live login either: `displaced/`, the
        // rescue path, `vault/`, dead temp files and the log, against every registered
        // provider's live files.
        self.refuse_live_in_targets(plan, &rows)?;
        self.refuse_live_in_envelope(plan, &rows, store.as_deref())?;
        let unlisted = match plan.provider {
            // §6.3: as `remove` would, before anything is deleted.
            Some(_) => {
                if let Some(row) = rows.first() {
                    if let Err(e) = self.rescue_paths_for(&row.id) {
                        return Err(EngineError::RescueUnlistable {
                            path: e.path,
                            detail: e.detail,
                        });
                    }
                }
                UnlistedRescues::Refuse
            }
            None => UnlistedRescues::Skip,
        };
        // 5, once nothing refuses: what recovery could not settle goes, with its warning.
        let warnings = self.purge_leftover_journals(&providers)?;
        let mut report = PurgeReport {
            warnings,
            ..PurgeReport::default()
        };
        // 7.
        let mut unfinished = 0;
        for row in &rows {
            self.check_cancel()?;
            let has_profile = fs::symlink_metadata(profile_path(&self.env, &row.id)).is_ok();
            match self.purge_account(row, unlisted, &providers) {
                Ok((rescues, warning)) => {
                    tracing::info!(
                        account = %row.id,
                        position = row.position,
                        provider = self.registered_id(&row.provider),
                        "purged an account"
                    );
                    report.rescues += rescues;
                    report.warnings.extend(warning);
                    report.accounts.push(account_of(row, has_profile));
                }
                Err(e) if e.signal().is_some() => return Err(e),
                Err(e) => {
                    unfinished += 1;
                    report.failures.push((
                        format!("{} #{} ({})", row.provider, row.position, row.label),
                        e.to_string(),
                    ));
                }
            }
            hooks::point(self, "purge-account-deleted")?;
        }
        // 8.
        for profile in &orphans {
            match self.delete_orphan(profile, &providers) {
                Ok(warning) => report.warnings.extend(warning),
                Err(e) => report
                    .failures
                    .push((profile.display().to_string(), e.to_string())),
            }
        }
        // 9. An account step 7 could not delete keeps its row, so the store's rows stay too.
        match &plan.provider {
            Some(p) => self.purge_provider_rest(p, unfinished, &mut report),
            None => self.purge_everything_else(plan.keychain_orphans, unfinished, &mut report),
        }
        Ok(report)
    }

    /// §10.5 step 3: each provider's engine lock, tried once and held by the caller. One that
    /// is held refuses, naming the pid its record holds (§11.1).
    fn hold_engine_locks(&self, providers: &[ProviderId]) -> Result<Vec<FlockGuard>, EngineError> {
        providers
            .iter()
            .map(|provider| {
                let path = engine_lock_path(&self.env, provider);
                match FlockGuard::try_lock(&path)? {
                    Some(lock) => Ok(lock),
                    None => Err(EngineError::EngineRunning {
                        provider: provider.to_string(),
                        pid: read_holder(&path).present().map(|r| r.pid),
                    }),
                }
            })
            .collect()
    }

    /// §10.5 step 7: one account, under its account lock, re-checked as `remove` checks it
    /// (not session-owned, no split profile), deleted exactly as `remove` deletes one (§10.3), or, of a provider this build does not
    /// register, as `remove_unregistered` does. Returns how many rescue files went, and a
    /// warning for the user, if any.
    fn purge_account(
        &self,
        row: &AccountRow,
        unlisted: UnlistedRescues,
        providers: &[ProviderId],
    ) -> Result<(usize, Option<String>), EngineError> {
        let lock = AccountLock::acquire(&self.env, &row.id, AccountLock::WAIT)?;
        match self.provider(&row.provider) {
            Ok(p) => {
                self.refuse_session_owned(p.as_ref(), row)?;
                self.refuse_profile_split(p.as_ref(), row)?;
                self.refuse_live_profile_item(p.as_ref(), row)?;
                Ok((self.remove_locked(row, &lock, unlisted)?, None))
            }
            Err(_) => {
                self.refuse_unregistered_in_use(row, providers)?;
                self.refuse_real_copies(&profile_path(&self.env, &row.id))?;
                self.refuse_live_files_in_unregistered(row, providers)?;
                self.remove_unregistered(row, &lock, unlisted)
            }
        }
    }

    /// Before an account of a provider this build does not register has its profile directory
    /// deleted: no registered provider's live login has its files inside it (the provider that
    /// kept the profile cannot be asked, so every one with sessions is).
    fn refuse_live_files_in_unregistered(
        &self,
        row: &AccountRow,
        providers: &[ProviderId],
    ) -> Result<(), EngineError> {
        let profile = profile_path(&self.env, &row.id);
        if fs::symlink_metadata(&profile).is_err() {
            return Ok(());
        }
        for p in self.judges(&profile, providers) {
            self.refuse_live_files_at(p.as_ref(), &profile)?;
        }
        Ok(())
    }

    /// `--provider P` (§10.5): a provider this build registers, or one the store still holds
    /// an account of, since purge is the way out of a state tagteam cannot repair. Any other
    /// name is `unknown-provider`.
    fn purgeable(&self, store: Option<&Store>, provider: &ProviderId) -> Result<(), EngineError> {
        let unknown = match self.provider(provider) {
            Ok(_) => return Ok(()),
            Err(e) => e,
        };
        match store {
            Some(s) if !s.accounts(provider)?.is_empty() => Ok(()),
            _ => Err(unknown),
        }
    }

    /// §10.5 step 6 for an account of a provider this build does not register: its profile is
    /// judged as an orphaned profile whose marker names such a provider is (`judges`), and one
    /// in use refuses as `session-owned`, naming the account.
    fn refuse_unregistered_in_use(
        &self,
        row: &AccountRow,
        providers: &[ProviderId],
    ) -> Result<(), EngineError> {
        let profile = profile_path(&self.env, &row.id);
        for p in self.judges(&profile, providers) {
            let state = self.session_state_at(p.as_ref(), &profile);
            if matches!(state, SessionState::Unreadable { .. }) {
                tracing::warn!(
                    position = row.position,
                    account = %row.id,
                    "a session reservation or record of the account's profile could not be read; the account counts as session-owned"
                );
            }
            if state.owned() {
                return Err(session_owned_error(row, &state));
            }
        }
        Ok(())
    }

    /// §10.5 step 7 for an account of a provider this build does not register: `remove_locked`'s
    /// steps (§10.3) without the provider's. A pending replacement is not reconciled, since only
    /// the provider can fingerprint the vault, and the account is deleted either way. The vault
    /// entry, the rescue files and the session profile go (its links removed as links), then the
    /// row. The credential item the provider keeps for the profile (§12.2) cannot be named, so
    /// it is left, and the warning returned says so. Returns how many rescue files went.
    fn remove_unregistered(
        &self,
        row: &AccountRow,
        lock: &AccountLock,
        unlisted: UnlistedRescues,
    ) -> Result<(usize, Option<String>), EngineError> {
        let rescues = match self.rescue_paths_for(&row.id) {
            Ok(paths) => paths,
            Err(_) if unlisted == UnlistedRescues::Skip => Vec::new(),
            Err(RescueUnlisted { path, detail }) => {
                return Err(EngineError::RescueUnlistable { path, detail });
            }
        };
        // Asked again where the deletions are, as `remove_locked` asks: the vault's files, and
        // the profile against every registered provider (its own cannot be asked).
        self.refuse_live_vault_files(row)?;
        let profile = profile_path(&self.env, &row.id);
        if fs::symlink_metadata(&profile).is_ok() {
            self.refuse_live_at_all(&profile)?;
        }
        self.vault.delete(lock)?;
        for path in &rescues {
            self.delete_rescue(path)?;
        }
        let had_profile = match fs::symlink_metadata(&profile) {
            Ok(_) => {
                remove_path(&profile)?;
                true
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        self.store()?.delete_account(&row.id)?;
        self.event(&row.provider, "remove", Some(&row.id), None)?;
        if !had_profile {
            return Ok((rescues.len(), None));
        }
        tracing::warn!(
            account = %row.id,
            position = row.position,
            "purged the session profile of an account whose provider this build does not register; the credential item that provider keeps for it cannot be named, and may remain"
        );
        let warning = format!(
            "{} #{} ({}): {} is a provider this build does not register, so the credential item it keeps for the account's session profile cannot be named; the profile was deleted, and that item may remain",
            row.provider, row.position, row.label, row.provider
        );
        Ok((rescues.len(), Some(warning)))
    }

    /// The IDs of `provider`'s displaced entries, from their rows: a file with no row names no
    /// provider, so only a full purge deletes it.
    fn displaced_of(&self, provider: &ProviderId) -> Result<Vec<String>, EngineError> {
        Ok(self
            .displaced()?
            .entries
            .into_iter()
            .filter(|e| e.provider.as_ref() == Some(provider))
            .map(|e| e.id)
            .collect())
    }

    /// §10.5 step 6: the directories and links under `sessions/` that are no stored account's
    /// profile (their name is no account's ID), and that this purge affects: every one in a
    /// full purge; under `--provider P`, those whose marker names P or cannot be read.
    fn orphan_profiles(
        &self,
        owned: &BTreeSet<String>,
        provider: Option<&ProviderId>,
    ) -> Result<Vec<PathBuf>, EngineError> {
        let dir = self.env.data_dir().join("sessions");
        let listing = match fs::read_dir(&dir) {
            Ok(l) => l,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut orphans = Vec::new();
        for entry in listing {
            let entry = entry?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| owned.contains(n))
            {
                continue;
            }
            let kind = entry.file_type()?;
            if !kind.is_dir() && !kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            let affected = match provider {
                None => true,
                Some(p) => match ProfileMarker::read(&path) {
                    Read::Present(marker) => &marker.provider == p,
                    Read::Absent | Read::Unreadable(_) => true,
                },
            };
            if affected {
                orphans.push(path);
            }
        }
        orphans.sort();
        Ok(orphans)
    }

    /// The providers that judge an orphaned profile, or the profile of an account whose provider
    /// this build does not register: the one its marker names, when this build registers it
    /// with sessions, else every affected provider with sessions, since any of them may have
    /// run it. An affected provider this build does not register cannot be asked, so every
    /// registered provider with sessions judges in its place.
    fn judges(&self, profile: &Path, providers: &[ProviderId]) -> Vec<Arc<dyn Provider>> {
        if let Read::Present(marker) = ProfileMarker::read(profile) {
            if let Ok(p) = self.provider(&marker.provider) {
                if p.capabilities().sessions {
                    return vec![p];
                }
            }
        }
        let mut judges: Vec<Arc<dyn Provider>> = Vec::new();
        for id in providers {
            let candidates = match self.provider(id) {
                Ok(p) => vec![p],
                Err(_) => self.registry.all().to_vec(),
            };
            for p in candidates {
                if p.capabilities().sessions && !judges.iter().any(|j| j.id() == p.id()) {
                    judges.push(p);
                }
            }
        }
        judges
    }

    /// §10.5 step 6: an orphaned profile with a live launch reservation, or a session record
    /// that is live or cannot be read (§12.5, §12.6), refuses the purge.
    fn refuse_orphan_in_use(
        &self,
        profile: &Path,
        providers: &[ProviderId],
    ) -> Result<(), EngineError> {
        for p in self.judges(profile, providers) {
            let state = self.session_state_at(p.as_ref(), profile);
            if matches!(state, SessionState::Unreadable { .. }) {
                tracing::warn!(
                    "a session reservation or record of {} could not be read; it counts as in use",
                    Self::profile_label(profile)
                );
            }
            if state.owned() {
                return Err(EngineError::OrphanSessionRunning {
                    profile: profile.to_path_buf(),
                    owner: Box::new(SessionOwner::of(&state)),
                });
            }
        }
        Ok(())
    }

    /// §10.3 Guard's split check (§12.2) for an orphaned profile: a readable marker naming a
    /// provider this build registers lets that provider's share lists judge it, as `remove`
    /// judges an account's profile. With a marker that cannot be read, or one naming a provider
    /// this build does not register, no share list is known, so every registered provider's
    /// must-share entries are checked, and a real one refuses (`refuse_real_copies`).
    fn refuse_orphan_split(&self, profile: &Path) -> Result<(), EngineError> {
        if let Read::Present(marker) = ProfileMarker::read(profile) {
            if let Ok(p) = self.provider(&marker.provider) {
                return self.refuse_split_at(p.as_ref(), profile, |_| true);
            }
        }
        self.refuse_real_copies(profile)
    }

    /// §10.5 step 8: the hashed item its marker's spelling names (§12.2), or, when the marker
    /// cannot be read, or names an account the store still holds (so the profile is a copy of
    /// that account's, never its own), the item each judging provider names from the profile's
    /// canonical path; then the entry itself, its links removed as links. A path that does not
    /// resolve names no item, so its item is skipped with a warning. So is the item of a marker
    /// naming a provider this build does not register, and the warning returned says so. An item
    /// that is the live login's is never deleted (§10.5): the orphan is left, and the refusal
    /// is returned as its failure. An entry that is itself a link is not a profile directory: no
    /// item is named through it, only the link is removed, and the warning says so.
    fn delete_orphan(
        &self,
        profile: &Path,
        providers: &[ProviderId],
    ) -> Result<Option<String>, EngineError> {
        let meta = match fs::symlink_metadata(profile) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        // Asked again just before it goes, as step 7 asks each account again.
        self.refuse_orphan_split(profile)?;
        // §10.5 step 6: an orphaned profile is a directory under `sessions/`. A link there is
        // not one, so no Keychain item is named through it, by its marker or by the path it
        // leads to: the link goes, and what it led to stays as it is.
        if meta.file_type().is_symlink() {
            // The link goes at its own location, and the live login may go through it.
            for p in self.judges(profile, providers) {
                self.refuse_live_files_at(p.as_ref(), profile)?;
            }
            fs::remove_file(profile)?;
            tracing::warn!(
                "{} is a link, not a profile directory; the link was removed and no Keychain item was deleted through it",
                Self::profile_label(profile)
            );
            return Ok(Some(format!(
                "{} is a link, not a profile directory; the link was removed and no Keychain item was deleted through it",
                profile.display()
            )));
        }
        let (items, warning) = self.orphan_items(profile, providers)?;
        if warning.is_some() {
            tracing::warn!(
                "{} names a provider this build does not register; the credential item that provider keeps for it cannot be named, and may remain",
                Self::profile_label(profile)
            );
        }
        // The live login's checks, again at the moment of deletion (a second line: the preflight
        // ran them already, so this fires only on a race).
        self.refuse_orphan_items_live(&items)?;
        self.refuse_orphan_items_stored(&items)?;
        for p in self.judges(profile, providers) {
            self.refuse_live_files_at(p.as_ref(), profile)?;
        }
        for (p, spelling) in &items {
            p.delete_profile_credential(&self.env, profile, spelling)?;
        }
        // `remove_dir_all` removes a symlink inside the profile as a link, never following it.
        if meta.is_dir() {
            fs::remove_dir_all(profile)?;
        } else {
            fs::remove_file(profile)?;
        }
        tracing::info!("deleted {}", Self::profile_label(profile));
        Ok(warning)
    }

    /// The Keychain items an orphan entry (a real directory) is given, and a warning when its
    /// marker names a provider this build does not register (§10.5 step 8).
    fn orphan_items(
        &self,
        profile: &Path,
        providers: &[ProviderId],
    ) -> Result<(OrphanItems, Option<String>), EngineError> {
        let mut warning = None;
        let mut items: OrphanItems = Vec::new();
        match ProfileMarker::read(profile) {
            Read::Present(marker) => match self.provider(&marker.provider) {
                Ok(p) if !self.store_holds(&marker.account_id)? => {
                    items.push((p, marker.config_dir));
                }
                // A copy of a stored account's profile: its marker's spelling is that
                // account's item, which is not this entry's to delete.
                Ok(_) => self.items_by_path(profile, providers, &mut items),
                Err(_) => {
                    warning = Some(format!(
                        "{} names {}, a provider this build does not register, so the credential item it keeps for that session profile cannot be named; the profile was deleted, and that item may remain",
                        profile.display(),
                        marker.provider
                    ));
                }
            },
            Read::Absent | Read::Unreadable(_) => {
                self.items_by_path(profile, providers, &mut items)
            }
        }
        Ok((items, warning))
    }

    /// The live login's item is never an orphan's to delete: an orphan can be a link to the live
    /// config directory, or spell it another way.
    fn refuse_orphan_items_live(
        &self,
        items: &[(Arc<dyn Provider>, String)],
    ) -> Result<(), EngineError> {
        for (p, spelling) in items {
            if p.live_item_spelling(&self.env).as_deref() == Some(spelling.as_str()) {
                let var = p.session_dir_var().unwrap_or("the home variable");
                return Err(EngineError::Io(io::Error::other(format!(
                    "the Keychain item it names is the live login's, since the environment ({var}, or the provider's override of it) names the same directory; it was left as it is, and so was the profile (a purge never deletes the live login)"
                ))));
            }
        }
        Ok(())
    }

    /// Nor is a stored account's: an orphan can be a link to, or a copy of, an account's profile.
    /// Left to the moment of deletion, where it leaves that one entry (the rest of the purge
    /// goes on), unlike the live login's, which refuses the whole purge.
    fn refuse_orphan_items_stored(
        &self,
        items: &[(Arc<dyn Provider>, String)],
    ) -> Result<(), EngineError> {
        let stored = self.stored_spellings()?;
        for (_, spelling) in items {
            if stored.contains(spelling) {
                return Err(EngineError::Io(io::Error::other(
                    "the Keychain item it names belongs to a stored account, since this entry leads to that account's profile; it was left as it is, and so was the entry",
                )));
            }
        }
        Ok(())
    }

    /// §10.5: refuses the purge when anything `target` is, or holds, is a registered provider's
    /// live login (`refuse_live_files_at` for each provider with a live login to protect).
    fn refuse_live_at_all(&self, target: &Path) -> Result<(), EngineError> {
        for p in self.registry.all() {
            self.refuse_live_files_at(p.as_ref(), target)?;
        }
        Ok(())
    }

    /// The temp files of §9.5's atomic writer in `dir` whose writer the process port finds gone:
    /// what `remove_dead_temp_files` deletes. A directory that cannot be listed has none to name
    /// here (its deletion reports the failure).
    fn dead_temp_files(&self, dir: &Path) -> Vec<PathBuf> {
        let Ok(listing) = fs::read_dir(dir) else {
            return Vec::new();
        };
        listing
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(temp_writer_pid)
                    .is_some_and(|pid| {
                        entry.file_type().is_ok_and(|t| t.is_file())
                            && self.process.exists(pid) == Some(false)
                    })
            })
            .map(|entry| entry.path())
            .collect()
    }

    /// The log and its rotations (§14.2).
    fn log_paths(&self) -> Vec<PathBuf> {
        let log = self.env.log_file();
        let rotations = LOG_ROTATIONS.iter().map(|suffix| {
            let mut path = log.clone().into_os_string();
            path.push(suffix);
            PathBuf::from(path)
        });
        std::iter::once(log.clone()).chain(rotations).collect()
    }

    /// Every filesystem entry besides profiles and orphans that this purge would delete, by
    /// scope: a full purge's `displaced/`, rescue path, `vault/` (Linux), dead temp files and
    /// log; a `--provider` purge's displaced files and its accounts' rescue files. The store
    /// file (emptied in place) and the lock files (kept) are not deleted, so not listed.
    fn deletion_targets(&self, plan: &PurgePlan, rows: &[AccountRow]) -> Vec<PathBuf> {
        let data = self.env.data_dir();
        let mut targets = Vec::new();
        match &plan.provider {
            None => {
                targets.push(data.join("displaced"));
                targets.push(self.rescue_dir());
                if let Some(vault) = self.vault.dir() {
                    targets.push(vault.to_path_buf());
                }
                for dir in [
                    data.clone(),
                    data.join("sessions"),
                    self.env.config_dir(),
                    self.env.state_dir(),
                ] {
                    targets.extend(self.dead_temp_files(&dir));
                }
                targets.extend(self.log_paths());
            }
            Some(provider) => {
                for id in self.displaced_of(provider).unwrap_or_default() {
                    targets.push(data.join("displaced").join(format!("{id}.json")));
                }
                for row in rows {
                    targets.extend(self.rescue_paths_for(&row.id).unwrap_or_default());
                }
            }
        }
        targets
    }

    /// The deletion-time recheck for `--provider`'s displaced files (the preflight ran it).
    fn refuse_live_in_displaced_files(&self, ids: &[String]) -> Result<(), EngineError> {
        let dir = self.env.data_dir().join("displaced");
        for id in ids {
            self.refuse_live_at_all(&dir.join(format!("{id}.json")))?;
        }
        Ok(())
    }

    /// Step 6, the envelope: purge can delete only inside tagteam's data directory and, for a
    /// full purge, the log's directory (and the config and state directories' temp files, which
    /// `refuse_live_in_targets` names). So no registered provider's live file may resolve to
    /// anything there, whatever it is called, EXCEPT inside the `sessions/<id>` profile of an
    /// account this purge leaves untouched (another provider's, under `--provider P`), or the
    /// directories on the way to one. Every path `trace_path` touches is judged by filesystem
    /// identity, never spelling. This closes what a list of targets keeps missing (an account's
    /// vault file, say); a failure to resolve refuses.
    fn refuse_live_in_envelope(
        &self,
        plan: &PurgePlan,
        rows: &[AccountRow],
        store: Option<&Store>,
    ) -> Result<(), EngineError> {
        let unresolved = || {
            EngineError::Io(io::Error::other(
                "a path the live login lives at could not be resolved, so it cannot be told whether this purge would delete it; nothing was deleted (tagteam never deletes the live login)",
            ))
        };
        let refused = || {
            EngineError::Io(io::Error::other(
                "the live login's files are inside what this would delete, since the environment names them; nothing was deleted (tagteam never deletes the live login)",
            ))
        };
        // The roots are every directory purge deletes in, by what each RESOLVES to
        // (`metadata` follows links): `vault/`, `displaced/` and `sessions/` may be links to
        // somewhere else entirely, and the rescue path is whatever it is.
        let mut walls: Vec<(u64, u64)> = Vec::new();
        let data = self.env.data_dir();
        let mut wall_dirs = vec![
            data.clone(),
            data.join("vault"),
            data.join("displaced"),
            data.join("sessions"),
            self.rescue_dir(),
        ];
        if let Some(vault) = self.vault.dir() {
            wall_dirs.push(vault.to_path_buf());
        }
        if plan.provider.is_none() {
            wall_dirs.extend(self.env.log_file().parent().map(Path::to_path_buf));
        }
        for dir in &wall_dirs {
            match fs::metadata(dir) {
                Ok(meta) => walls.push(identity(&meta)),
                Err(e) if absent(&e) => {}
                Err(_) => return Err(unresolved()),
            }
        }
        if walls.is_empty() {
            return Ok(());
        }
        // Every affected account's vault files (`vault/<id>.json`, `.prev.json`), whoever its
        // provider is: what `vault.delete` unlinks.
        for row in rows {
            self.refuse_live_vault_files(row)?;
        }
        // What the purge leaves alone: the profiles of the accounts it does not affect, and the
        // directories above each, which a path passes through to reach one.
        let affected: BTreeSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        let mut spared: Vec<(u64, u64)> = Vec::new();
        let mut on_the_way: Vec<(u64, u64)> = Vec::new();
        if let Some(store) = store {
            for row in store.all_accounts()? {
                if affected.contains(row.id.as_str()) {
                    continue;
                }
                let profile = profile_path(&self.env, &row.id);
                match fs::symlink_metadata(&profile) {
                    Ok(meta) => spared.push(identity(&meta)),
                    Err(e) if absent(&e) => continue,
                    Err(_) => return Err(unresolved()),
                }
                if let Ok(meta) = fs::metadata(&profile) {
                    spared.push(identity(&meta));
                }
                for above in profile.ancestors().skip(1) {
                    match fs::metadata(above) {
                        Ok(meta) => on_the_way.push(identity(&meta)),
                        Err(e) if absent(&e) => break,
                        Err(_) => return Err(unresolved()),
                    }
                }
            }
        }
        for p in self.registry.all() {
            let surface = p.identity_surface(&self.env);
            let files = surface
                .credential_files
                .into_iter()
                .chain(surface.json_keys.into_iter().map(|(file, _)| file));
            for file in files {
                for touched in trace_path(&file).map_err(|_| unresolved())? {
                    // Each ancestor as itself, and, for a link, as what it leads to: a link
                    // to a root counts as the root.
                    let mut chain = Vec::new();
                    for ancestor in touched.ancestors() {
                        match fs::symlink_metadata(ancestor) {
                            Ok(meta) => chain.push(identity(&meta)),
                            Err(e) if absent(&e) => break,
                            Err(_) => return Err(unresolved()),
                        }
                        if let Ok(meta) = fs::metadata(ancestor) {
                            chain.push(identity(&meta));
                        }
                    }
                    let inside = chain.iter().any(|id| walls.contains(id));
                    let spared_path = chain.iter().any(|id| spared.contains(id))
                        || chain.first().is_some_and(|id| on_the_way.contains(id));
                    if inside && !spared_path {
                        tracing::warn!(
                            "the live login's files are inside what a purge would delete; nothing was deleted"
                        );
                        return Err(refused());
                    }
                }
            }
        }
        Ok(())
    }

    /// Step 6: refuses the whole purge when any of `deletion_targets` holds a registered
    /// provider's live login. The error names the entry for the user (never a log).
    fn refuse_live_in_targets(
        &self,
        plan: &PurgePlan,
        rows: &[AccountRow],
    ) -> Result<(), EngineError> {
        for target in self.deletion_targets(plan, rows) {
            self.refuse_live_at_all(&target).map_err(|e| {
                EngineError::Io(io::Error::other(format!("{}: {e}", target.display())))
            })?;
        }
        Ok(())
    }

    /// §10.5 step 6, for an orphan entry: refuses the purge when deleting it would delete or
    /// break the live login, by Keychain item (a real directory only: no item is named through
    /// a link) and by the live login's files. Nothing is deleted.
    fn refuse_orphan_live(
        &self,
        profile: &Path,
        providers: &[ProviderId],
    ) -> Result<(), EngineError> {
        let meta = match fs::symlink_metadata(profile) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        if !meta.file_type().is_symlink() {
            let (items, _) = self.orphan_items(profile, providers)?;
            self.refuse_orphan_items_live(&items)?;
        }
        for p in self.judges(profile, providers) {
            self.refuse_live_files_at(p.as_ref(), profile)?;
        }
        Ok(())
    }

    /// How a log line names an entry of `sessions/` no account owns, never by its path (§14.2):
    /// the data directory may sit under a name the user chose (`XDG_DATA_HOME`), and the entry
    /// may carry any name at all. So it is "the orphaned profile of account <ID>" only when its
    /// name is an ID as tagteam makes one (a hyphenated UUID), and otherwise an unrecognized
    /// entry.
    fn profile_label(profile: &Path) -> String {
        let name = profile.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if uuid::Uuid::parse_str(name).is_ok_and(|u| u.hyphenated().to_string() == name) {
            format!("the orphaned profile of account {name}")
        } else {
            "an unrecognized profile entry".to_owned()
        }
    }

    /// Whether the store holds the account `id`, of any provider.
    fn store_holds(&self, id: &AccountId) -> Result<bool, EngineError> {
        Ok(match self.existing_store()? {
            Some(store) => store.account(id)?.is_some(),
            None => false,
        })
    }

    /// The spellings that name a stored account's profile item: the one its marker records,
    /// and the canonical path of its profile as each registered provider with sessions spells
    /// it. Fails closed: a profile that cannot be resolved, for any cause but "not there",
    /// refuses (`delete_orphan` leaves the entry), since its spelling cannot be told apart.
    fn stored_spellings(&self) -> Result<BTreeSet<String>, EngineError> {
        let mut spellings = BTreeSet::new();
        let Some(store) = self.existing_store()? else {
            return Ok(spellings);
        };
        for row in store.all_accounts()? {
            let profile = profile_path(&self.env, &row.id);
            if let Read::Present(marker) = ProfileMarker::read(&profile) {
                spellings.insert(marker.config_dir);
            }
            match canonical_profile_path(&profile) {
                Ok(canonical) => {
                    for p in self.registry.all() {
                        if p.capabilities().sessions {
                            spellings.insert(p.profile_spelling(&canonical));
                        }
                    }
                }
                // No profile, so no item named by its path.
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) => {}
                // Any other cause drops a stored account's spelling and opens the guard:
                // the entry is left as it is. A fixed text (§14.2): no path, no error text.
                Err(_) => {
                    return Err(EngineError::Io(io::Error::other(
                        "a stored account's profile could not be resolved, so this entry cannot be told apart from it; it was left as it is",
                    )));
                }
            }
        }
        Ok(spellings)
    }

    /// The items an orphan with no usable marker is given: each judging provider's, named from
    /// the profile's canonical path. A path that does not resolve names none.
    fn items_by_path(
        &self,
        profile: &Path,
        providers: &[ProviderId],
        items: &mut Vec<(Arc<dyn Provider>, String)>,
    ) {
        match canonical_profile_path(profile) {
            Ok(canonical) => {
                for p in self.judges(profile, providers) {
                    let spelling = p.profile_spelling(&canonical);
                    items.push((p, spelling));
                }
            }
            Err(e) => tracing::warn!(
                "{} has no readable marker and does not resolve ({e}); no Keychain item can be named for it",
                Self::profile_label(profile)
            ),
        }
    }

    /// §10.5 with `--provider P`, once its accounts are gone: P's displaced entries, each file
    /// before its row (§6.3), then its other rows. Its `usage_requests` rows stay. With
    /// `unfinished` accounts left, P's rows stay with them, and a failure says so.
    fn purge_provider_rest(
        &self,
        provider: &ProviderId,
        unfinished: usize,
        report: &mut PurgeReport,
    ) {
        // R-T4-purgeerror: `purge_displaced` stops at its first failure, and the entries before
        // it stay deleted, so they count; the failure is reported with its cause.
        match self.displaced_of(provider) {
            Ok(ids) if ids.is_empty() => {}
            Ok(ids) => match self
                .refuse_live_in_displaced_files(&ids)
                .map_err(|cause| PurgeError {
                    cause,
                    deleted: Vec::new(),
                })
                .and_then(|()| self.purge_displaced(&ids))
            {
                Ok(deleted) => report.displaced = deleted.len(),
                Err(PurgeError { cause, deleted }) => {
                    report.displaced = deleted.len();
                    report
                        .failures
                        .push(("displaced credentials".into(), cause.to_string()));
                }
            },
            Err(e) => report
                .failures
                .push(("displaced credentials".into(), e.to_string())),
        }
        let what = format!("the store's rows for {provider}");
        if unfinished > 0 {
            let again = format!("tagteam purge --provider {provider}");
            report
                .failures
                .push((what, store_kept(unfinished, "", &again)));
        } else {
            let rows = self.existing_store().and_then(|store| match store {
                Some(s) => Ok(s.delete_provider_rows(provider)?),
                None => Ok(()),
            });
            if let Err(e) = rows {
                report.failures.push((what, e.to_string()));
            }
        }
        tracing::info!(
            provider = self.registered_id(provider),
            accounts = report.accounts.len(),
            rescues = report.rescues,
            displaced = report.displaced,
            failures = report.failures.len(),
            "purge finished"
        );
    }

    /// §10.5 without `--provider`, once every account is gone, in the spec's order. With
    /// `unfinished` accounts left (step 7 could not delete them), what they hold stays with
    /// them: the store's rows, which running purge again finds them by; the vault's leftovers,
    /// which may be their entries; and the `rescue` path, since a newer generation deleted from
    /// it while the vault keeps the consumed one is what vault first rules out (§10.5 "A purge
    /// that stops part-way"). One failure says so. `displaced/`, dead temp files and the log
    /// are no account's, and go either way. The log goes last, and nothing is logged after it:
    /// a line would create it again (Decision 9).
    fn purge_everything_else(
        &self,
        keychain_orphans: bool,
        unfinished: usize,
        report: &mut PurgeReport,
    ) {
        let data = self.env.data_dir();
        if unfinished == 0 {
            self.sweep_vault(keychain_orphans, report);
            // The rescue path whatever it is (§6.3).
            let rescue = self.rescue_dir();
            // Each deletion asks again (a second line: the preflight ran it, so only a race
            // trips it).
            match self
                .refuse_live_at_all(&rescue)
                .map_err(io::Error::other)
                .and_then(|()| remove_path(&rescue))
            {
                Ok(n) => report.rescues += n,
                Err(e) => report
                    .failures
                    .push((rescue.display().to_string(), e.to_string())),
            }
        }
        let displaced = data.join("displaced");
        match self
            .refuse_live_at_all(&displaced)
            .map_err(io::Error::other)
            .and_then(|()| remove_path(&displaced))
        {
            Ok(n) => report.displaced += n,
            Err(e) => report
                .failures
                .push((displaced.display().to_string(), e.to_string())),
        }
        // §9.5's temp files whose writer is gone, in tagteam's own directories only.
        self.remove_temp_files_in(
            &[
                data.clone(),
                data.join("sessions"),
                self.env.config_dir(),
                self.env.state_dir(),
            ],
            report,
        );
        // The store's rows, in place (Decision 8), unless an account still needs them.
        if unfinished > 0 {
            let kept = store_kept(
                unfinished,
                ", with the rescue path and the vault's leftovers",
                "tagteam purge",
            );
            report.failures.push(("the store".into(), kept));
        } else {
            match self.existing_store() {
                Ok(None) => report.store_emptied = true,
                Ok(Some(store)) => match store.empty_all() {
                    Ok(()) => report.store_emptied = true,
                    Err(e) => report.failures.push(("the store".into(), e.to_string())),
                },
                Err(e) => report.failures.push(("the store".into(), e.to_string())),
            }
        }
        self.delete_log(report);
    }

    /// §10.5 for a full purge that found no data directory: only what lies outside one, the
    /// Keychain's leftovers, dead temp files in the config and state directories, and the log.
    /// It takes no lock, and never opens, creates or empties a store: a data directory that
    /// appeared since the check (an `add`) is the guarded path's, and `None` hands it over. A
    /// `--provider` purge has nothing to delete.
    fn purge_without_data(&self, plan: &PurgePlan) -> Result<Option<PurgeReport>, EngineError> {
        let mut report = PurgeReport::default();
        hooks::point(self, "purge-without-data-dir")?;
        if self.env.data_dir().try_exists()? {
            return Ok(None);
        }
        if plan.provider.is_some() {
            return Ok(Some(report));
        }
        // Outside the data directory: dead temp files and the log, guarded as the full purge's.
        for dir in [self.env.config_dir(), self.env.state_dir()] {
            for target in self.dead_temp_files(&dir) {
                self.refuse_live_at_all(&target).map_err(|e| {
                    EngineError::Io(io::Error::other(format!("{}: {e}", target.display())))
                })?;
            }
        }
        for target in self.log_paths() {
            self.refuse_live_at_all(&target).map_err(|e| {
                EngineError::Io(io::Error::other(format!("{}: {e}", target.display())))
            })?;
        }
        // A vault kept in a directory keeps it in the data directory: nothing is there.
        if self.vault.dir().is_none() {
            self.sweep_vault(plan.keychain_orphans, &mut report);
        }
        self.remove_temp_files_in(&[self.env.config_dir(), self.env.state_dir()], &mut report);
        report.store_emptied = true;
        self.delete_log(&mut report);
        Ok(Some(report))
    }

    /// The vault's leftovers: on macOS the `tagteam` items no account of this store names,
    /// which every data directory on the Mac shares, so only `--keychain-orphans` deletes
    /// them; on Linux `vault/`, which is this data directory's alone.
    fn sweep_vault(&self, keychain_orphans: bool, report: &mut PurgeReport) {
        match self.vault.sweep(keychain_orphans) {
            Ok(Leftovers::None) => {}
            Ok(Leftovers::Shared) => report.warnings.push(KEYCHAIN_LEFTOVERS.into()),
            Ok(Leftovers::Unknown(detail)) => report.warnings.push(format!(
                "could not tell whether the Keychain still holds `tagteam` items that no account names ({detail})"
            )),
            Err(e) => report.failures.push(("the vault".into(), e.to_string())),
        }
    }

    /// `remove_dead_temp_files` over each of `dirs`, a failure reported for its directory.
    fn remove_temp_files_in(&self, dirs: &[PathBuf], report: &mut PurgeReport) {
        for dir in dirs {
            if let Err(e) = self.remove_dead_temp_files(dir) {
                report
                    .failures
                    .push((dir.display().to_string(), e.to_string()));
            }
        }
    }

    /// The totals, logged, then the log and its rotations, last (§14.2). Its rotation lock is
    /// a lock file, and stays.
    fn delete_log(&self, report: &mut PurgeReport) {
        tracing::info!(
            accounts = report.accounts.len(),
            rescues = report.rescues,
            displaced = report.displaced,
            store_emptied = report.store_emptied,
            failures = report.failures.len(),
            "purge finished; deleting the log"
        );
        for path in self.log_paths() {
            if let Err(e) = self.refuse_live_at_all(&path) {
                report
                    .failures
                    .push((path.display().to_string(), e.to_string()));
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => report
                    .failures
                    .push((path.display().to_string(), e.to_string())),
            }
        }
    }

    /// Deletes `dir`'s temp files of §9.5's atomic writer whose writer the process port finds
    /// gone. A writer that is live, or cannot be judged, may still publish its file.
    fn remove_dead_temp_files(&self, dir: &Path) -> io::Result<()> {
        let listing = match fs::read_dir(dir) {
            Ok(l) => l,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        for entry in listing {
            let entry = entry?;
            let Some(pid) = entry.file_name().to_str().and_then(temp_writer_pid) else {
                continue;
            };
            if !entry.file_type()?.is_file() || self.process.exists(pid) != Some(false) {
                continue;
            }
            self.refuse_live_at_all(&entry.path())
                .map_err(io::Error::other)?;
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_a_path_counts_what_it_held() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("rescue");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("a.json"), "x").unwrap();
        fs::write(dir.join("b.json"), "x").unwrap();
        assert_eq!(entries_in(&dir), 2);
        assert_eq!(remove_path(&dir).unwrap(), 2);
        assert!(!dir.exists());
        fs::write(&dir, "a file where the directory was").unwrap();
        assert_eq!(entries_in(&dir), 1);
        assert_eq!(remove_path(&dir).unwrap(), 1);
        assert_eq!(remove_path(&dir).unwrap(), 0, "absent is done");
        assert_eq!(entries_in(&dir), 0);
        let target = d.path().join("kept");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &dir).unwrap();
        assert_eq!(remove_path(&dir).unwrap(), 1, "a link is removed as a link");
        assert!(target.exists());
    }
}
