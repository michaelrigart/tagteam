//! §13.3's import (Decision 13). Pass 1 validates every account before anything is read or
//! written; pass 2 writes them one at a time under `MutationGuard` and each account's lock. An
//! existing login is replaced only through §12.5's explicit replacement, and a new one is
//! created as `add` creates one. No account ID is read from the file, and the store's active
//! account is never set from it (B.65). No message quotes a value of the file.

use std::collections::HashSet;

use tagteam_core::validate::normalize_alias;
use tagteam_core::{AccountId, IdentityKey, ProviderId};
use tagteam_provider::{Provider, Read, StoredLogin};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::lifecycle::{LoginSource, Prepared, check_identity_conflict};
use crate::store::{AccountRow, Store, StoreError};
use crate::transfer::ImportRecord;

const ALREADY_STORED: &str = "already stored; pass --force to replace it";

/// What became of one account of the file (§13.3's `outcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Created,
    Replaced,
    Skipped,
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Created => "created",
            Outcome::Replaced => "replaced",
            Outcome::Skipped => "skipped",
            Outcome::Failed => "failed",
        }
    }
}

/// One account of the file, as it ended: where it is stored now, or, failed, where the file
/// put it. `email` is the login's email, or its label for a provider whose logins have none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    pub provider: ProviderId,
    pub position: u32,
    pub email: String,
    pub outcome: Outcome,
    pub message: String,
}

/// §13.3's report, in the file's order, with the notices the import owes the user.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportReport {
    pub accounts: Vec<Imported>,
    pub warnings: Vec<String>,
}

impl ImportReport {
    /// The command exits 1 when any account failed (§13.3).
    pub fn any_failed(&self) -> bool {
        self.accounts.iter().any(|a| a.outcome == Outcome::Failed)
    }
}

/// A record pass 1 accepted: its login as the provider rebuilt it, and its normalized alias.
struct Valid {
    record: ImportRecord,
    login: StoredLogin,
    key: IdentityKey,
    alias: Option<String>,
}

fn email_of(row: &AccountRow) -> String {
    row.email.clone().unwrap_or_else(|| row.label.clone())
}

/// What became of a stored account, logged by its ID and position only (§14.2).
fn imported(row: &AccountRow, outcome: Outcome, message: impl Into<String>) -> Imported {
    tracing::info!(
        account = %row.id,
        position = row.position,
        outcome = outcome.as_str(),
        "imported an account"
    );
    Imported {
        provider: row.provider.clone(),
        position: row.position,
        email: email_of(row),
        outcome,
        message: message.into(),
    }
}

/// A pass-1 refusal, naming the account by its place in the file.
fn refused(n: usize, why: impl std::fmt::Display) -> EngineError {
    EngineError::InvalidInput(format!("account {n} of the file: {why}"))
}

impl Engine {
    /// §13.3. Refused inside a run shell (§12.8), and while an interrupted switch for a provider
    /// in the file cannot be decided (§9.6). Pass 1 refuses the whole file on any invalid
    /// account, before anything exists. Pass 2 holds `MutationGuard` throughout and takes each
    /// account's lock in turn; a failure on one account is reported, and the others go on.
    pub fn import(
        &self,
        records: Vec<ImportRecord>,
        force: bool,
    ) -> Result<ImportReport, EngineError> {
        self.refuse_inside_run_shell()?;
        let valid = self.validate_import(records)?;
        if valid.is_empty() {
            return Ok(ImportReport::default());
        }
        let mut providers: Vec<ProviderId> =
            valid.iter().map(|v| v.record.provider.clone()).collect();
        providers.sort();
        providers.dedup();
        let _guard = self.guard_or_refuse_each(&providers, "cli")?;
        let store = self.store()?;
        let mut report = ImportReport::default();
        for v in &valid {
            match self.import_one(&store, v, force, &mut report.warnings) {
                Ok(done) => report.accounts.push(done),
                Err(e) if e.signal().is_some() => return Err(e),
                Err(e) => {
                    tracing::warn!(
                        position = v.record.position,
                        kind = e.kind(),
                        "an account could not be imported"
                    );
                    let identity = &v.login.identity;
                    report.accounts.push(Imported {
                        provider: v.record.provider.clone(),
                        position: v.record.position,
                        email: identity
                            .email
                            .clone()
                            .unwrap_or_else(|| identity.label.clone()),
                        outcome: Outcome::Failed,
                        message: e.to_string(),
                    });
                }
            }
        }
        Ok(report)
    }

    /// Pass 1 (§13.3, B.34): every record's provider is registered and validates its identity
    /// and credential; its kind is one of the provider's and is what the credential is; its
    /// alias follows §10.3's rules; no two records are one login of a provider, and no two
    /// share an alias. Nothing is read from the store, and nothing is written. A refusal
    /// names the account by its place in the file and never quotes a value of it.
    fn validate_import(&self, records: Vec<ImportRecord>) -> Result<Vec<Valid>, EngineError> {
        let mut logins = HashSet::new();
        let mut aliases = HashSet::new();
        records
            .into_iter()
            .enumerate()
            .map(|(i, record)| {
                let n = i + 1;
                let p = self.provider(&record.provider)?;
                if record.position == 0 {
                    return Err(refused(n, "its position must be at least 1"));
                }
                let login = p
                    .import_login(&record.identity, &record.credential)
                    .map_err(|e| refused(n, e))?;
                if let Some(kind) = &record.kind {
                    if !p.credential_kinds().contains(&kind.as_str()) {
                        return Err(refused(
                            n,
                            format!("its kind is not a {} credential kind", p.display_name()),
                        ));
                    }
                    if *kind != login.kind {
                        return Err(refused(
                            n,
                            format!("its kind does not match its credential's, {}", login.kind),
                        ));
                    }
                }
                let alias = record
                    .alias
                    .as_deref()
                    .map(normalize_alias)
                    .transpose()
                    .map_err(|e| refused(n, format!("its alias: {e}")))?;
                let key = p.identity_key(&login.identity);
                if !logins.insert((record.provider.clone(), key.clone())) {
                    return Err(refused(n, "it is the same login as an earlier account"));
                }
                if let Some(a) = &alias {
                    if !aliases.insert(a.clone()) {
                        return Err(refused(n, "its alias is an earlier account's"));
                    }
                }
                Ok(Valid {
                    record,
                    login,
                    key,
                    alias,
                })
            })
            .collect()
    }

    /// Pass 2 for one record.
    fn import_one(
        &self,
        store: &Store,
        v: &Valid,
        force: bool,
        warnings: &mut Vec<String>,
    ) -> Result<Imported, EngineError> {
        let p = self.provider(&v.record.provider)?;
        match store.find_by_identity_key(&v.record.provider, v.key.as_str())? {
            Some(row) => self.import_over(store, p.as_ref(), &row.id, v, force, warnings),
            None => self.import_new(store, p.as_ref(), v, warnings),
        }
    }

    /// An existing identity (§13.3): skipped unless `force`, or quarantined; otherwise replaced
    /// as an explicit replacement (§12.5 steps 1–3), with the evidence recorded from the live
    /// identity as `add-token` records it. It keeps its local position, alias and `disabled`.
    fn import_over(
        &self,
        store: &Store,
        p: &dyn Provider,
        id: &AccountId,
        v: &Valid,
        force: bool,
        warnings: &mut Vec<String>,
    ) -> Result<Imported, EngineError> {
        let lock = self.lock_account(id)?;
        let row = store.account(id)?.ok_or(StoreError::NoSuchAccount)?;
        let quarantined = row.quarantine_reason.is_some();
        if !force && !quarantined {
            return Ok(imported(&row, Outcome::Skipped, ALREADY_STORED));
        }
        let identity = &v.login.identity;
        check_identity_conflict(
            Some(&row),
            identity.account_uuid.as_deref(),
            &identity.label,
        )?;
        // §12.5: whether the live identity names the account, read under its lock. One that
        // cannot be read leaves the evidence undecidable, so nothing is written (B.1).
        let live_names_account = match p.live_identity(&self.env) {
            Read::Present(live) => p.identity_key(&live) == v.key,
            Read::Absent => false,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let prep = Prepared {
            existing: Some(row.clone()),
            occupant: None,
            id: row.id.clone(),
        };
        let (account, _) = self.commit_login(
            store,
            p,
            &row.provider,
            &prep,
            std::slice::from_ref(&lock),
            identity,
            &v.login.kind,
            &v.login.secret,
            None,
            None,
            LoginSource {
                from_live: false,
                live_names_account,
                event: "import",
                added_at: None,
            },
        )?;
        if store.live_store_stale(&account)? {
            warnings.push(format!(
                "position {n} is {app}'s live login, which keeps its old login until you run `tagteam switch {n} --force{flag}`",
                n = account.position,
                app = p.display_name(),
                flag = self.provider_flag(&account.provider),
            ));
        }
        if self.session_state(p, &account)?.owned() {
            warnings.push(format!(
                "position {} is in use by a `tagteam run` session, which keeps its login until it exits; the next `run` starts with the imported one",
                account.position
            ));
        }
        let message = if quarantined {
            "replaced; its quarantine was cleared"
        } else {
            "replaced"
        };
        Ok(imported(&account, Outcome::Replaced, message))
    }

    /// A new identity (§13.3), created as `add` creates one: the store row before the vault
    /// entry (§10.1), at its exported position if that is free, else at the provider's next
    /// position (§6.1). Its alias and `disabled` come from the file; an alias another account
    /// holds here is dropped.
    fn import_new(
        &self,
        store: &Store,
        p: &dyn Provider,
        v: &Valid,
        warnings: &mut Vec<String>,
    ) -> Result<Imported, EngineError> {
        let provider = &v.record.provider;
        let wanted = v.record.position;
        let limit = store.next_position(provider)?.saturating_sub(1).max(99);
        let position = (wanted <= limit && store.find_by_position(provider, wanted)?.is_none())
            .then_some(wanted);
        let alias = match &v.alias {
            Some(a) if store.find_by_alias(a)?.is_some() => {
                warnings.push(format!(
                    "the alias of the file's account at position {wanted} is another account's here, so the account was imported without it"
                ));
                None
            }
            other => other.clone(),
        };
        let prep = Prepared {
            existing: None,
            occupant: None,
            id: AccountId::from_string(uuid::Uuid::now_v7().to_string()),
        };
        let lock = self.lock_account(&prep.id)?;
        let (account, _) = self.commit_login(
            store,
            p,
            provider,
            &prep,
            std::slice::from_ref(&lock),
            &v.login.identity,
            &v.login.kind,
            &v.login.secret,
            position,
            alias.as_deref(),
            LoginSource {
                from_live: false,
                live_names_account: false,
                event: "import",
                added_at: v.record.added_at,
            },
        )?;
        if v.record.disabled {
            store.set_disabled(&account.id, true)?;
        }
        let account = store
            .account(&account.id)?
            .ok_or(StoreError::NoSuchAccount)?;
        let message = if account.position == wanted {
            "added".to_owned()
        } else {
            format!(
                "added at position {}: {wanted} is not free here",
                account.position
            )
        };
        Ok(imported(&account, Outcome::Created, message))
    }

    /// ` --provider <id>` for a provider other than the default, for a command a notice names.
    fn provider_flag(&self, provider: &ProviderId) -> String {
        if provider == &self.default_provider {
            String::new()
        } else {
            format!(" --provider {provider}")
        }
    }
}
