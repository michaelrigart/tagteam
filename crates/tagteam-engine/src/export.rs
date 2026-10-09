//! §13.3's export (Decision 12): each account's newest generation, read where its lineage
//! advances, under `MutationGuard` and then the account's lock, one account at a time. It sends
//! no request, and never writes the live store or a profile. The CLI encrypts and writes what
//! it returns once every lock is released.

use std::collections::BTreeMap;
use std::fmt;

use tagteam_core::{AccountId, ProvenanceVerdict, ProviderId, provenance};
use tagteam_provider::profile::{ProfileMarker, Seed};
use tagteam_provider::{Credential, MutationGuard, Provenance, Provider, Read, StoredLogin};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::provenance::{ProfileCheck, identity_drifted};
use crate::rescue::{RescueEntry, RescueFile};
use crate::session::SessionState;
use crate::store::{AccountRow, JournalRow};
use crate::transfer::{self, EnvelopeAccount, Generation, PendingRescue, Pick};

// §13.3's broken accounts, as the skipped list and the refusal name them.
const NO_VAULT: &str = "it has no stored credential";
const NO_IDENTITY: &str = "its stored identity cannot be read";
const QUARANTINED: &str = "it is quarantined: its login was rejected, so log in again";
const INTERRUPTED: &str = "an interrupted switch that recovery cannot decide names it";
const PROFILE_CONFLICT: &str =
    "its session profile and the vault both moved since they last agreed";
const NO_LIVE_CREDENTIAL: &str = "the live login it names has no credential";
const DEGRADED_LIVE: &str =
    "the live credential could be read only from its file, which may be out of date";
const WIPED: &str = "Claude Code wiped the copy in use after its login was rejected";
const NO_PROFILE_MARKER: &str = "its session profile's marker cannot be read";
const NO_PROFILE_CREDENTIAL: &str = "its running session holds no credential";
const DEGRADED_PROFILE: &str =
    "its session's credential could be read only from its file, which may be out of date";
const PROFILE_LACKS_REFRESH: &str =
    "its session's credential has no refresh token, while the vault's has one";
const NO_SEED: &str = "its running session has no seed to compare its credential against";
const UNKNOWN_PROVIDER: &str = "its provider is not in this build";
const ROTATION_UNNAMED: &str = "its running session rotated the login but names no identity, so the rotation cannot be told to be the account's";
const NO_TOKEN: &str = "its credential holds no token, so it could not be imported";

/// What `export` exports (§13.3).
#[derive(Debug, Clone, Default)]
pub struct ExportRequest {
    /// `--account`, resolved by the CLI (§10.4). `None` exports every account.
    pub accounts: Option<Vec<AccountId>>,
    /// `--provider`, narrowing an export of every account.
    pub provider: Option<ProviderId>,
    /// `--full`: the provider's whole credential (§13.3).
    pub full: bool,
}

/// Where an account's exported generation was read (§13.3's `source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Vault,
    Live,
    Profile,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Vault => "vault",
            Source::Live => "live",
            Source::Profile => "profile",
        }
    }
}

/// One exported account, as the store has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Exported {
    pub row: AccountRow,
    pub source: Source,
    /// The live login or session-owned: this machine goes on refreshing it (§13.3 "In use here").
    pub in_use: bool,
    /// A login that refreshes, which the file hands over rather than copies (§13.3).
    pub refreshes: bool,
}

/// An account a bulk export left out, and why (§13.3 "Broken accounts").
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub row: AccountRow,
    pub reason: String,
}

/// The plaintext envelope (§13.3) and what went into it, in provider and position order. The
/// envelope holds every exported credential, so `Debug` shows its length only.
pub struct ExportResult {
    pub envelope: Vec<u8>,
    pub accounts: Vec<Exported>,
    pub skipped: Vec<Skipped>,
}

impl fmt::Debug for ExportResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExportResult")
            .field("envelope", &format_args!("<{} bytes>", self.envelope.len()))
            .field("accounts", &self.accounts)
            .field("skipped", &self.skipped)
            .finish()
    }
}

/// What one account came to.
enum One {
    /// Removed since the list was read.
    Gone,
    Exported(Box<(Exported, EnvelopeAccount)>),
    Broken(Box<(AccountRow, EngineError)>),
}

fn broken<T>(row: &AccountRow, reason: impl Into<String>) -> Result<T, EngineError> {
    Err(EngineError::AccountBroken {
        position: row.position,
        reason: reason.into(),
    })
}

/// Whether `j`, or a row a forced switch superseded in it, names `id` (§9.6).
fn journal_names(j: &JournalRow, id: &AccountId) -> bool {
    &j.to_id == id
        || j.from_id.as_ref() == Some(id)
        || j.prior.as_deref().is_some_and(|p| journal_names(p, id))
}

/// An error that ends the whole export rather than one account: an interruption (§14.1), or
/// the store itself failing.
fn ends_the_export(e: &EngineError) -> bool {
    e.signal().is_some() || matches!(e, EngineError::Store(_))
}

/// A fresh, complete read of the copy in use, or the reason it is not one.
fn fresh_copy(
    row: &AccountRow,
    read: Read<Credential>,
    missing: &str,
    degraded: &str,
) -> Result<Vec<u8>, EngineError> {
    match read {
        Read::Present(c) if c.provenance() == Provenance::Degraded => broken(row, degraded),
        Read::Present(c) if !c.is_empty() => Ok(c.bytes().to_vec()),
        Read::Present(_) | Read::Absent => broken(row, missing),
        Read::Unreadable(e) => broken(row, format!("its credential in use cannot be read ({e})")),
    }
}

impl Engine {
    /// §13.3. Every account of `req`, one at a time in ascending ID order (§4.3), each under
    /// `MutationGuard` and then its account lock, both released before the next. Recovery runs
    /// under the guard as for any holder (§9.6), from fingerprints alone, since export sends no
    /// request (§7.6). Nothing is created when there is no store (§5).
    ///
    /// A broken account is skipped with its reason; named by `--account`, it is the command's
    /// error, and nothing is returned to write. It works inside a run shell, where the live
    /// login is the default home's (§12.8).
    pub fn export(&self, req: &ExportRequest) -> Result<ExportResult, EngineError> {
        let explicit = req.accounts.is_some();
        let mut ids: Vec<AccountId> = match (&req.accounts, self.existing_store()?) {
            (Some(ids), Some(_)) => ids.clone(),
            (Some(ids), None) => {
                let first = ids.first().map(AccountId::to_string).unwrap_or_default();
                return Err(EngineError::NoSuchAccount(first));
            }
            (None, Some(store)) => store
                .all_accounts()?
                .into_iter()
                .filter(|r| req.provider.as_ref().is_none_or(|p| &r.provider == p))
                .map(|r| r.id)
                .collect(),
            (None, None) => Vec::new(),
        };
        ids.sort();
        ids.dedup();
        let (mut done, mut skipped) = (Vec::new(), Vec::new());
        for id in &ids {
            match self.export_one(id, req.full)? {
                One::Gone if explicit => return Err(EngineError::NoSuchAccount(id.to_string())),
                One::Gone => {}
                One::Exported(both) => done.push(*both),
                One::Broken(broken) => {
                    let (row, e) = *broken;
                    tracing::warn!(
                        account = %row.id,
                        position = row.position,
                        kind = e.kind(),
                        "an account was not exported"
                    );
                    let reason = match e {
                        EngineError::AccountBroken { reason, .. } => reason,
                        e => e.to_string(),
                    };
                    if explicit {
                        return Err(EngineError::AccountBroken {
                            position: row.position,
                            reason,
                        });
                    }
                    skipped.push(Skipped { row, reason });
                }
            }
        }
        done.sort_by(|(a, _), (b, _)| {
            (&a.row.provider, a.row.position).cmp(&(&b.row.provider, b.row.position))
        });
        skipped.sort_by(|a, b| {
            (&a.row.provider, a.row.position).cmp(&(&b.row.provider, b.row.position))
        });
        let providers: Vec<ProviderId> = done.iter().map(|(e, _)| e.row.provider.clone()).collect();
        let active = self.live_positions(&providers)?;
        let (accounts, payloads): (Vec<Exported>, Vec<EnvelopeAccount>) = done.into_iter().unzip();
        Ok(ExportResult {
            envelope: transfer::envelope(self.now_ms(), &active, &payloads),
            accounts,
            skipped,
        })
    }

    /// The live login's position for each of `providers` that tagteam manages, for the
    /// envelope's informational `active` (§13.3).
    fn live_positions(
        &self,
        providers: &[ProviderId],
    ) -> Result<BTreeMap<String, u32>, EngineError> {
        let mut out = BTreeMap::new();
        let Some(store) = self.existing_store()? else {
            return Ok(out);
        };
        for id in providers {
            let p = self.provider(id)?;
            if let Read::Present(live) = p.live_identity(&self.env) {
                if let Some(row) = store.find_by_identity_key(id, p.identity_key(&live).as_str())? {
                    out.insert(id.to_string(), row.position);
                }
            }
        }
        Ok(out)
    }

    /// One account, under the guard and its lock, which are released on return.
    fn export_one(&self, id: &AccountId, full: bool) -> Result<One, EngineError> {
        let guard = self.metadata_guard()?;
        let Some(store) = self.existing_store()? else {
            return Ok(One::Gone);
        };
        let Some(row) = store.account(id)? else {
            return Ok(One::Gone);
        };
        let outcome = self.lock_account(id).and_then(|lock| {
            // Taking the lock reconciled a pending replacement (§12.5): read the row again.
            let row = store
                .account(id)?
                .ok_or(EngineError::NoSuchAccount(id.to_string()))?;
            self.export_locked(&row, &lock, &guard, full)
                .map(|exported| (row, exported))
        });
        match outcome {
            Ok((row, (exported, account))) => {
                tracing::info!(
                    account = %row.id,
                    position = row.position,
                    source = exported.source.as_str(),
                    "exported an account"
                );
                Ok(One::Exported(Box::new((exported, account))))
            }
            Err(EngineError::NoSuchAccount(_)) => Ok(One::Gone),
            Err(e) if ends_the_export(&e) => Err(e),
            Err(e) => Ok(One::Broken(Box::new((row, e)))),
        }
    }

    /// §13.3 for one account whose lock is held.
    fn export_locked(
        &self,
        row: &AccountRow,
        lock: &AccountLock,
        guard: &MutationGuard,
        full: bool,
    ) -> Result<(Exported, EnvelopeAccount), EngineError> {
        let Ok(p) = self.provider(&row.provider) else {
            return broken(row, UNKNOWN_PROVIDER);
        };
        let p = p.as_ref();
        if row.quarantine_reason.is_some() {
            return broken(row, QUARANTINED);
        }
        if self
            .store()?
            .journal(&row.provider)?
            .is_some_and(|j| journal_names(&j, &row.id))
        {
            return broken(row, INTERRUPTED);
        }
        let Ok(identity) = p.parse_identity(&row.identity_json) else {
            return broken(row, NO_IDENTITY);
        };
        let (secret, source, in_use) = self.newest_generation(p, row, lock, guard)?;
        // B.64, and the rule `import_login` applies on the other side: a wiped generation, or
        // one no fingerprint can be taken of, is no login to hand over.
        if p.is_wiped(&secret) || p.fingerprint(&secret).is_none() {
            return broken(row, NO_TOKEN);
        }
        let (identity, credential) = p
            .export_login(
                &StoredLogin {
                    kind: row.kind.clone(),
                    secret,
                    identity,
                },
                full,
            )
            .or_else(|e| broken(row, e.to_string()))?;
        Ok((
            Exported {
                row: row.clone(),
                source,
                in_use,
                refreshes: p.kind_traits(&row.kind).refreshable,
            },
            EnvelopeAccount {
                provider: row.provider.clone(),
                position: row.position,
                kind: row.kind.clone(),
                label: row.label.clone(),
                alias: row.alias.clone(),
                disabled: row.disabled,
                added_at: row.added_at,
                identity,
                credential,
            },
        ))
    }

    /// §13.3 "Which generation is exported", with whether the account is in use here.
    fn newest_generation(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        guard: &MutationGuard,
    ) -> Result<(Vec<u8>, Source, bool), EngineError> {
        let refreshes = p.kind_traits(&row.kind).refreshable;
        let live_names = match p.live_identity(&self.env) {
            Read::Present(live) => p.identity_key(&live).as_str() == row.identity_key,
            Read::Absent => false,
            // Which account is live decides which copy is newest, so a refreshing account
            // cannot be exported blind (B.1).
            Read::Unreadable(e) if refreshes => {
                return broken(row, format!("the live login cannot be told apart ({e})"));
            }
            Read::Unreadable(_) => false,
        };
        let state = self.session_state(p, row)?;
        let in_use = live_names || state.owned();
        if !refreshes {
            // A kind that never refreshes has one generation: the vault's.
            return Ok((self.export_vault_generation(row)?, Source::Vault, in_use));
        }
        if live_names && !self.store()?.live_store_stale(row)? {
            let (bytes, source) = self.newest_live_generation(p, row, guard)?;
            return Ok((bytes, source, in_use));
        }
        if state.owned() {
            let (bytes, source) = self.newest_profile_generation(p, row, &state, guard)?;
            return Ok((bytes, source, in_use));
        }
        // Any other account, a stale-marked live one included (the replacement wins): the
        // vault, after the work every holder of its lock does first (§6.2, §12.5).
        self.settle_rescues(p, row, lock)?;
        match self.apply_provenance(p, row, lock)? {
            ProfileCheck::Conflict => return broken(row, PROFILE_CONFLICT),
            ProfileCheck::Unreadable(detail) => {
                return broken(
                    row,
                    format!("its session profile cannot be read ({detail})"),
                );
            }
            _ => {}
        }
        Ok((self.export_vault_generation(row)?, Source::Vault, in_use))
    }

    fn export_vault_generation(&self, row: &AccountRow) -> Result<Vec<u8>, EngineError> {
        match self.vault.read(&row.id) {
            Read::Present(b) => Ok(b),
            Read::Absent => broken(row, NO_VAULT),
            Read::Unreadable(e) => {
                broken(row, format!("its stored credential cannot be read ({e})"))
            }
        }
    }

    /// The account's rescues (§6.3); one that cannot be read may hold its newest generation.
    fn pending_rescues(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<(Vec<RescueEntry>, Vec<PendingRescue>), EngineError> {
        let mut entries = Vec::new();
        for file in self.rescues_for(&row.id) {
            match file {
                RescueFile::Entry(e) => entries.push(e),
                RescueFile::Unreadable { path, detail } => {
                    return broken(
                        row,
                        format!(
                            "a rescue file cannot be read ({}: {detail})",
                            path.display()
                        ),
                    );
                }
            }
        }
        let pending = entries
            .iter()
            .map(|e| PendingRescue {
                fp: p.fingerprint(&e.credential),
                predecessor: e.predecessor_fp.clone(),
            })
            .collect();
        Ok((entries, pending))
    }

    /// The live login (§13.3): its credential read fresh under the provider's credential
    /// locks, so a refresh in flight completes first, then `transfer::newest_live`.
    fn newest_live_generation(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        guard: &MutationGuard,
    ) -> Result<(Vec<u8>, Source), EngineError> {
        let held = p.lock_credentials(&self.env, guard, p.live_lock_budget())?;
        let live = p.read_live_auth(&self.env).credential;
        drop(held);
        let live = fresh_copy(row, live, NO_LIVE_CREDENTIAL, DEGRADED_LIVE)?;
        let Some(live_fp) = p.fingerprint(&live).filter(|_| !p.is_wiped(&live)) else {
            return broken(row, WIPED);
        };
        let vault = self.export_vault_generation(row)?;
        let Some(vault_fp) = p.fingerprint(&vault) else {
            return broken(row, NO_VAULT);
        };
        let prev = match self.vault.read_prev(&row.id) {
            Read::Present(b) => p.fingerprint(&b),
            Read::Absent => None,
            Read::Unreadable(e) => {
                return broken(
                    row,
                    format!("its previous stored credential cannot be read ({e})"),
                );
            }
        };
        let (entries, pending) = self.pending_rescues(p, row)?;
        let live_gen = Generation {
            fp: live_fp,
            full: p.has_refresh_token(&live),
        };
        match transfer::newest_live(&live_gen, &vault_fp, prev.as_ref(), &pending) {
            Pick::Live => Ok((live, Source::Live)),
            Pick::Vault => Ok((vault, Source::Vault)),
            Pick::Rescue(i) => Ok((entries[i].credential.clone(), Source::Vault)),
            Pick::Broken(reason) => broken(row, reason),
        }
    }

    /// A session-owned account (§13.3): the profile's credential read as the agent reads it,
    /// under the profile's own credential locks, held against the vault's through the seed
    /// (§12.5). A profile whose identity drifted (names another login) is ignored, and so is
    /// a stale-marked one's credential, whatever it holds: the vault's generation is taken. An
    /// identity that is absent is no drift: the table runs, and only a rotation needs it, as for
    /// every holder of the account's lock (`apply_provenance`).
    fn newest_profile_generation(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        state: &SessionState,
        guard: &MutationGuard,
    ) -> Result<(Vec<u8>, Source), EngineError> {
        let profile = state
            .profile()
            .expect("a session-owned account has a profile");
        let spelling = match ProfileMarker::read(profile) {
            Read::Present(m) if m.account_id == row.id && m.provider == row.provider => {
                m.config_dir
            }
            _ => return broken(row, NO_PROFILE_MARKER),
        };
        let vault = self.export_vault_generation(row)?;
        let (entries, pending) = self.pending_rescues(p, row)?;
        let vault_or_successor = |vault: Vec<u8>| -> (Vec<u8>, Source) {
            let successor = p
                .fingerprint(&vault)
                .and_then(|fp| transfer::successor(&fp, &pending));
            match successor {
                Some(i) => (entries[i].credential.clone(), Source::Vault),
                None => (vault, Source::Vault),
            }
        };
        let identity_absent = match p.profile_identity(&self.env, profile) {
            Read::Present(login) if !identity_drifted(&login, row) => false,
            Read::Present(_) => return Ok(vault_or_successor(vault)),
            Read::Absent => true,
            Read::Unreadable(e) => {
                return broken(row, format!("its session's identity cannot be read ({e})"));
            }
        };
        // A stale-marked profile exports the vault's generation before its own credential is
        // looked at, wiped or not: the replacement wins (§13.3, §12.5).
        let seed = match Seed::read(profile) {
            Read::Present(seed) if seed.login_epoch != row.login_epoch => {
                return Ok(vault_or_successor(vault));
            }
            Read::Present(seed) => Some(seed),
            Read::Absent => None,
            Read::Unreadable(e) => {
                return broken(row, format!("its session's seed cannot be read ({e})"));
            }
        };
        let read = p.read_profile_credential_settled(&self.env, profile, &spelling, guard)?;
        let held = fresh_copy(row, read, NO_PROFILE_CREDENTIAL, DEGRADED_PROFILE)?;
        let (Some(p_fp), Some(v_fp)) = (
            p.fingerprint(&held).filter(|_| !p.is_wiped(&held)),
            p.fingerprint(&vault),
        ) else {
            return broken(row, WIPED);
        };
        if !p.has_refresh_token(&held) && p.has_refresh_token(&vault) {
            return broken(row, PROFILE_LACKS_REFRESH);
        }
        let Some(seed) = seed else {
            if p_fp == v_fp {
                return Ok(vault_or_successor(vault));
            }
            return broken(row, NO_SEED);
        };
        // Not stale-marked: that returned above.
        match provenance(p_fp.as_str(), v_fp.as_str(), &seed.seed_fp, false) {
            // The profile rotated the vault's generation, which is consumed: nothing says the
            // rotation is the account's, so neither copy can be exported (M4a's Decision 9).
            ProvenanceVerdict::Capture if identity_absent => broken(row, ROTATION_UNNAMED),
            ProvenanceVerdict::Capture => Ok((held, Source::Profile)),
            ProvenanceVerdict::Conflict => broken(row, PROFILE_CONFLICT),
            ProvenanceVerdict::InStep { .. }
            | ProvenanceVerdict::VaultMovedOn
            | ProvenanceVerdict::ReplacementWins => Ok(vault_or_successor(vault)),
        }
    }
}
