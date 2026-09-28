use tagteam_core::validate::{is_valid_email, normalize_alias};
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::{Credential, Identity, Provenance, Provider, Read};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{AccountRow, EventRow, LoginMeta, NewAccount, Store, StoreError};

pub struct AddOptions {
    pub provider: ProviderId,
    pub position: Option<u32>,
    pub alias: Option<String>,
    pub yes: bool,
}

pub struct AddTokenOptions {
    pub provider: ProviderId,
    pub token: String,
    pub position: Option<u32>,
    pub email: Option<String>,
    pub alias: Option<String>,
    pub yes: bool,
}

#[derive(Debug)]
pub struct AddOutcome {
    pub account: AccountRow,
    pub created: bool,
    pub notices: Vec<String>,
}

pub(crate) fn alias_arg(alias: Option<&str>) -> Result<Option<String>, EngineError> {
    alias
        .map(normalize_alias)
        .transpose()
        .map_err(|e| EngineError::InvalidInput(e.to_string()))
}

/// A move or add target must be within 1..=max(99, max(position)) (§6.1 Positions).
pub(crate) fn check_position(position: u32, max_existing: u32) -> Result<(), EngineError> {
    let limit = max_existing.max(99);
    if position == 0 || position > limit {
        return Err(EngineError::InvalidInput(format!(
            "positions run from 1 to {limit}"
        )));
    }
    Ok(())
}

/// Uuid first, org corroborating (§10.1 guard 2).
pub(crate) fn same_owner(a: &Identity, b: &Identity) -> bool {
    match (&a.account_uuid, &b.account_uuid) {
        (Some(x), Some(y)) => x == y && a.org_uuid == b.org_uuid,
        _ => a.email == b.email && a.org_uuid == b.org_uuid,
    }
}

/// The managed-key guard (§10.1 guard 1), with all three read states.
fn no_live_api_key(r: &Read<Vec<u8>>) -> Result<(), EngineError> {
    match r {
        Read::Present(_) => Err(EngineError::LiveApiKey),
        Read::Absent => Ok(()),
        Read::Unreadable(e) => Err(EngineError::Unreadable(e.clone())),
    }
}

fn live_fresh(r: Read<Credential>) -> Result<Credential, EngineError> {
    match r {
        Read::Present(c) if c.provenance() == Provenance::Degraded => {
            Err(EngineError::DegradedRead)
        }
        Read::Present(c) if c.is_empty() => Err(EngineError::NoLiveLogin),
        Read::Present(c) => Ok(c),
        Read::Absent => Err(EngineError::NoLiveLogin),
        Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
    }
}

fn alias_taken(e: StoreError) -> EngineError {
    match e {
        StoreError::AliasTaken(a) => {
            EngineError::InvalidInput(format!("the alias {a:?} is already taken"))
        }
        other => other.into(),
    }
}

/// What a login write will replace, decided before anything is mutated.
struct Prepared {
    existing: Option<AccountRow>,
    occupant: Option<AccountRow>,
    id: AccountId,
}

impl Engine {
    pub(crate) fn event(
        &self,
        provider: &ProviderId,
        kind: &str,
        from: Option<&AccountId>,
        to: Option<&AccountId>,
    ) -> Result<(), EngineError> {
        self.store()?.insert_event(&EventRow {
            at: self.now_ms(),
            provider: provider.clone(),
            kind: kind.to_owned(),
            from_id: from.cloned(),
            to_id: to.cloned(),
            trigger: None,
            source: "cli".into(),
            detail: None,
        })?;
        Ok(())
    }

    /// Deletes the vault entries (strict), then the row (which cascades). The caller holds the
    /// mutation lock and this account's lock. The live login is never touched.
    pub(crate) fn remove_locked(
        &self,
        row: &AccountRow,
        lock: &AccountLock,
    ) -> Result<(), EngineError> {
        self.vault.delete(lock)?;
        self.store()?.delete_account(&row.id)?;
        self.event(&row.provider, "remove", Some(&row.id), None)?;
        Ok(())
    }

    /// Pure validation, no mutation: which account is written, which occupant it would
    /// replace, and whether that is allowed.
    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &self,
        store: &Store,
        p: &dyn Provider,
        provider: &ProviderId,
        identity: &Identity,
        position: Option<u32>,
        alias: Option<&str>,
        yes: bool,
    ) -> Result<Prepared, EngineError> {
        let existing = store.find_by_identity_key(provider, p.identity_key(identity).as_str())?;
        // `update_login`/`finish_replacement` COALESCE a new account_uuid over a known one
        // (Task 16's review): a stored uuid that a different incoming uuid would silently
        // overwrite is refused instead.
        if let Some(e) = &existing {
            if let (Some(old), Some(new)) =
                (e.account_uuid.as_deref(), identity.account_uuid.as_deref())
            {
                if old != new {
                    return Err(EngineError::InvalidInput(format!(
                        "{} is already stored with a different account_uuid; refusing to overwrite it",
                        identity.label
                    )));
                }
            }
        }
        let mut occupant = None;
        if let Some(pos) = position {
            check_position(pos, store.next_position(provider)?.saturating_sub(1))?;
            occupant = store
                .find_by_position(provider, pos)?
                .filter(|o| existing.as_ref().map(|e| &e.id) != Some(&o.id));
            if let (Some(o), false) = (&occupant, yes) {
                return Err(EngineError::NeedsConfirmation {
                    position: pos,
                    occupant: o.label.clone(),
                });
            }
        }
        if let Some(a) = alias {
            if let Some(owner) = store.find_by_alias(a)? {
                let freed = [existing.as_ref(), occupant.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(|r| r.id == owner.id);
                if !freed {
                    return Err(EngineError::InvalidInput(format!(
                        "the alias {a:?} is already taken"
                    )));
                }
            }
        }
        let id = match &existing {
            Some(e) => e.id.clone(),
            None => AccountId::from_string(uuid::Uuid::now_v7().to_string()),
        };
        Ok(Prepared {
            existing,
            occupant,
            id,
        })
    }

    fn lock_prepared(&self, prep: &Prepared) -> Result<Vec<AccountLock>, EngineError> {
        let mut ids = vec![&prep.id];
        if let Some(o) = &prep.occupant {
            ids.push(&o.id);
        }
        self.lock_accounts(&ids)
    }

    /// Writes the login under the locks `prep` names. The replacement is persisted before the
    /// occupant it displaces is removed, so a failure never loses the occupant.
    #[allow(clippy::too_many_arguments)]
    fn commit_login(
        &self,
        store: &Store,
        p: &dyn Provider,
        provider: &ProviderId,
        prep: &Prepared,
        locks: &[AccountLock],
        identity: &Identity,
        kind: &str,
        secret: &[u8],
        position: Option<u32>,
        alias: Option<&str>,
    ) -> Result<(AccountRow, bool), EngineError> {
        let lock_for = |id: &AccountId| {
            locks
                .iter()
                .find(|l| l.id() == id)
                .expect("every written account is locked")
        };
        let fp = |b: &[u8]| p.fingerprint(b);
        let key = p.identity_key(identity);
        match &prep.existing {
            Some(row) => {
                // The metadata travels with the marker: if this process dies after the vault
                // write, the next lock holder installs it (§12.5).
                let new_fp = p
                    .fingerprint(secret)
                    .map(|f| f.as_str().to_owned())
                    .unwrap_or_default();
                let meta = LoginMeta {
                    identity_key: key.as_str(),
                    identity,
                    kind,
                    login_expires_at: p.login_expires_at(secret),
                };
                store.begin_replacement(&row.id, &new_fp, &meta)?;
                self.vault.store(lock_for(&row.id), secret, &fp)?;
                store.finish_replacement(&row.id)?;
            }
            None => {
                // At a free position first; it moves once any occupant is gone.
                store.insert_account(&NewAccount {
                    id: &prep.id,
                    provider,
                    position: store.next_position(provider)?,
                    identity_key: key.as_str(),
                    identity,
                    kind,
                    alias: None,
                    login_expires_at: p.login_expires_at(secret),
                    added_at: self.now_ms(),
                })?;
                if let Err(e) = self.vault.store(lock_for(&prep.id), secret, &fp) {
                    store.delete_account(&prep.id)?;
                    return Err(e.into());
                }
            }
        }
        if let Some(occupant) = &prep.occupant {
            self.remove_locked(occupant, lock_for(&occupant.id))?;
        }
        let current = store.account(&prep.id)?.ok_or(StoreError::NoSuchAccount)?;
        if let Some(pos) = position.filter(|pos| *pos != current.position) {
            store.move_to(&prep.id, pos)?;
        }
        if alias.is_some() {
            store.set_alias(&prep.id, alias).map_err(alias_taken)?;
        }
        self.event(provider, "add", None, Some(&prep.id))?;
        Ok((
            store.account(&prep.id)?.ok_or(StoreError::NoSuchAccount)?,
            prep.existing.is_none(),
        ))
    }

    /// §10.1: captures the live login.
    pub fn add_live(&self, opts: AddOptions) -> Result<AddOutcome, EngineError> {
        self.refuse_inside_run_shell()?;
        self.settle_or_refuse(&opts.provider)?;
        let p = self.provider(&opts.provider)?;
        let alias = alias_arg(opts.alias.as_deref())?;
        if let Some(pos) = opts.position {
            // Validated before anything exists (§5); `prepare` re-checks under the lock.
            let max = match self.existing_store()? {
                Some(s) => s.next_position(&opts.provider)?.saturating_sub(1),
                None => 0,
            };
            check_position(pos, max)?;
        }
        // 1. The live identity, read once.
        let identity = match p.live_identity(&self.env) {
            Read::Present(i) => i,
            Read::Absent => return Err(EngineError::NoLiveLogin),
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        // 2. The live credential, from the store CC would use; a degraded read is refused.
        let auth = p.read_live_auth(&self.env);
        no_live_api_key(&auth.managed_key)?;
        let cred = live_fresh(auth.credential)?;
        // Task 13's review: `classify` falls through to `setup_token` for any bytes it does
        // not recognise, a wiped credential included, so it has no "unrecognised" answer of
        // its own. Bytes with no fingerprint (which a wiped credential always is: neither
        // token is present) are refused here, before classification ever sees them.
        if p.fingerprint(cred.bytes()).is_none() {
            return Err(EngineError::NoLiveLogin);
        }
        // 3.2 Ownership, advisory and before any lock: never refreshes, never blocks on failure.
        let mut notices = Vec::new();
        match self.oracle.resolve(p.as_ref(), &cred) {
            Some(owner) if !same_owner(&owner, &identity) => {
                return Err(EngineError::OwnerMismatch {
                    expected: identity.label,
                    found: owner.label,
                });
            }
            Some(_) => {}
            None => notices.push(format!(
                "could not verify that the live credential belongs to {}",
                identity.label
            )),
        }
        let kind = p.classify(cred.bytes());
        // 4. Write, under the mutation lock, the account locks and then CC's live locks.
        let guard = self.mutation_guard()?;
        self.refuse_if_interrupted(&opts.provider)?;
        let store = self.store()?;
        let prep = self.prepare(
            &store,
            p.as_ref(),
            &opts.provider,
            &identity,
            opts.position,
            alias.as_deref(),
            opts.yes,
        )?;
        let accounts = self.lock_prepared(&prep)?;
        let live_locks = p.lock_live(&self.env, &guard)?;
        // 3.3 The capture must be the login verified above, on both auth axes: a switch or a
        // recovery may have moved it while this command waited for the locks.
        // The complete identity is compared, not just its key: an `accountUuid` or any other
        // `oauthAccount` field that changed means this is not the login that was verified.
        let now_identity = p.live_identity(&self.env).present();
        let now_auth = p.read_live_auth(&self.env);
        no_live_api_key(&now_auth.managed_key)?;
        let now = live_fresh(now_auth.credential)?;
        if now_identity.as_ref().map(|i| &i.raw) != Some(&identity.raw)
            || p.fingerprint(now.bytes()) != p.fingerprint(cred.bytes())
        {
            return Err(EngineError::LiveMoved);
        }
        let (account, created) = self.commit_login(
            &store,
            p.as_ref(),
            &opts.provider,
            &prep,
            &accounts,
            &identity,
            &kind,
            now.bytes(),
            opts.position,
            alias.as_deref(),
        )?;
        drop(live_locks);
        store.set_active(&opts.provider, Some(&account.id))?;
        Ok(AddOutcome {
            account,
            created,
            notices,
        })
    }

    /// §10.2: stores a token without touching the live login. No network calls. Everything
    /// that can be checked without the lock is checked before anything is created (§5).
    pub fn add_token(&self, opts: AddTokenOptions) -> Result<AddOutcome, EngineError> {
        self.refuse_inside_run_shell()?;
        self.settle_or_refuse(&opts.provider)?;
        let p = self.provider(&opts.provider)?;
        let alias = alias_arg(opts.alias.as_deref())?;
        if opts.token.trim().is_empty() {
            return Err(EngineError::InvalidInput("the token is empty".into()));
        }
        let (kind, secret) = p.token_secret(&opts.token);
        let prefix = if kind == "api_key" {
            "api-key"
        } else {
            "setup-token"
        };
        let email_for = |position: u32| {
            opts.email
                .clone()
                .unwrap_or_else(|| format!("{prefix}-{position}@token.local"))
        };
        let next = match self.existing_store()? {
            Some(s) => s.next_position(&opts.provider)?,
            None => 1,
        };
        if let Some(pos) = opts.position {
            check_position(pos, next.saturating_sub(1))?;
        }
        let email = email_for(opts.position.unwrap_or(next));
        if !is_valid_email(&email) {
            return Err(EngineError::InvalidInput(format!(
                "{email:?} is not a valid email address"
            )));
        }
        let _guard = self.mutation_guard()?;
        self.refuse_if_interrupted(&opts.provider)?;
        let store = self.store()?;
        // Under the lock the position is authoritative, so a default email follows it.
        let position = match opts.position {
            Some(pos) => pos,
            None => store.next_position(&opts.provider)?,
        };
        let identity = p.token_identity(&email_for(position));
        let prep = self.prepare(
            &store,
            p.as_ref(),
            &opts.provider,
            &identity,
            opts.position,
            alias.as_deref(),
            opts.yes,
        )?;
        let accounts = self.lock_prepared(&prep)?;
        // The different-kind collision (§10.2) is decided only now: taking the account lock
        // may have finished a pending replacement and changed the account's kind.
        if let Some(e) = &prep.existing {
            if let Some(existing) = store.account(&e.id)? {
                if existing.kind != kind {
                    return Err(EngineError::InvalidInput(format!(
                        "{} is already stored as a {} account",
                        identity.label, existing.kind
                    )));
                }
            }
        }
        let (account, created) = self.commit_login(
            &store,
            p.as_ref(),
            &opts.provider,
            &prep,
            &accounts,
            &identity,
            &kind,
            &secret,
            opts.position,
            alias.as_deref(),
        )?;
        Ok(AddOutcome {
            account,
            created,
            notices: vec![],
        })
    }
}
