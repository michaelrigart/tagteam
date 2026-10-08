use std::fs;
use std::io;

use tagteam_core::validate::{is_valid_email, normalize_alias};
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::profile::{ProfileMarker, canonical_profile_path, profile_path};
use tagteam_provider::{Credential, Identity, Provenance, Provider, Read};

use crate::account_lock::AccountLock;
use crate::engine::{Engine, Reconcile};
use crate::error::EngineError;
use crate::rescue::{RescueEntry, RescueFile};
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

/// Uuid first, org corroborating (§10.1 guard 2). Only meaningful once the caller has
/// already established that `a` (typically an oracle's answer) is itself resolved: §7.6
/// makes an identity with no non-empty `account_uuid` carry no attribution signal at all,
/// so that gate is applied by the caller before this is ever invoked (never inside it,
/// since `identity_key` lookups elsewhere pass a self-reported identity here as `b`, whose
/// own uuid may legitimately be unbackfilled yet).
pub(crate) fn same_owner(a: &Identity, b: &Identity) -> bool {
    match (&a.account_uuid, &b.account_uuid) {
        (Some(x), Some(y)) => x == y && a.org_uuid == b.org_uuid,
        _ => a.email == b.email && a.org_uuid == b.org_uuid,
    }
}

/// §7.6: an oracle answer counts as resolved only when its own `account_uuid` is a
/// non-empty string; without one it carries no attribution signal and is treated exactly
/// like no answer at all (the "could not verify" notice path), never compared by email.
fn oracle_resolved(owner: Option<Identity>) -> Option<Identity> {
    owner.filter(|o| o.account_uuid.as_deref().is_some_and(|u| !u.is_empty()))
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

/// §10.2: a token kind whose provider names no default email must be given `--email`.
fn no_default_email(kind: &str) -> EngineError {
    EngineError::InvalidInput(format!(
        "a {kind} token has no default email address; pass --email"
    ))
}

/// `update_login`/`finish_replacement` COALESCE a new account_uuid over a known one, so a
/// stored uuid that a different incoming uuid would overwrite is refused instead. Called
/// both by `prepare`'s pre-lock fast path and again, post-lock, by `add_live`/`add_token`:
/// reconciling a pending replacement under the account lock can install a uuid the pre-lock
/// read never saw, so the pre-lock check alone is not enough.
fn check_identity_conflict(
    row: Option<&AccountRow>,
    claimed_uuid: Option<&str>,
    label: &str,
) -> Result<(), EngineError> {
    let Some(row) = row else {
        return Ok(());
    };
    if let (Some(old), Some(new)) = (row.account_uuid.as_deref(), claimed_uuid) {
        if old != new {
            return Err(EngineError::IdentityConflict {
                label: label.to_owned(),
            });
        }
    }
    Ok(())
}

/// §10.2's default email, `<prefix>-<N>@token.local`, with N the position the token goes to,
/// or the next N no account holds when that identity is taken. A defaulted identity never
/// names an existing account, so a plain `add-token` never replaces one: only an explicit
/// `--email` does.
fn unused_token_identity(
    store: &Store,
    p: &dyn Provider,
    provider: &ProviderId,
    prefix: &str,
    position: u32,
) -> Result<Identity, EngineError> {
    let mut n = position;
    loop {
        let identity = p.token_identity(&format!("{prefix}-{n}@token.local"));
        let key = p.identity_key(&identity);
        if store
            .find_by_identity_key(provider, key.as_str())?
            .is_none()
        {
            return Ok(identity);
        }
        n += 1;
    }
}

/// What a login write will replace, decided before anything is mutated.
struct Prepared {
    existing: Option<AccountRow>,
    occupant: Option<AccountRow>,
    id: AccountId,
}

/// Where a login comes from, for the evidence an explicit replacement records (§12.5).
#[derive(Clone, Copy)]
struct LoginSource {
    /// Taken from the live store (`add`): finishing the replacement records its new epoch as
    /// the activation epoch (§10.1).
    from_live: bool,
    /// The live identity names the account (§12.5 "A replacement records its own evidence").
    live_names_account: bool,
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

    /// §10.3's order: the vault entries (strict), then the account's rescue files (§6.3: each
    /// holds a live refresh token, readable or not, and none is left behind), then the session
    /// profile (`remove_profile`), and last the row (which cascades). The vault goes first, so
    /// no generation older than a rescue or a rotated profile can outlive it: an account
    /// without a vault credential is never switched to, refreshed or launched. Anything that
    /// fails stops before the row goes, so the account stays listed and the remove can be run
    /// again; every delete treats an absent item as done. The caller holds the mutation lock and
    /// this account's lock, and has run `refuse_destroying` on it. The live login is never
    /// touched. A pending replacement is reconciled first (§6.2), but one that cannot be
    /// installed does not stop it: the account goes either way (§12.5).
    pub(crate) fn remove_locked(
        &self,
        row: &AccountRow,
        lock: &AccountLock,
    ) -> Result<(), EngineError> {
        self.reconcile_replacement_as(lock, Reconcile::Removing)?;
        let p = self.provider(&row.provider)?;
        self.vault.delete(lock)?;
        for rescue in self.rescues_for(&row.id) {
            let (RescueFile::Entry(RescueEntry { path, .. }) | RescueFile::Unreadable { path, .. }) =
                rescue;
            self.delete_rescue(&path)?;
        }
        self.remove_profile(p.as_ref(), row)?;
        self.store()?.delete_account(&row.id)?;
        self.event(&row.provider, "remove", Some(&row.id), None)?;
        Ok(())
    }

    /// §10.3 Guard, before `remove_locked` deletes anything of `row`: it is not session-owned
    /// (§12.5), and its profile holds no history that deleting it would delete or leave split
    /// (§12.2, `refuse_profile_split`). Either refusal leaves everything as it was.
    fn refuse_destroying(&self, p: &dyn Provider, row: &AccountRow) -> Result<(), EngineError> {
        self.refuse_session_owned(p, row)?;
        self.refuse_profile_split(p, row)
    }

    /// §10.3: deletes `row`'s session profile, if it has one. The agent's credential items for
    /// the spelling the marker records go first (§12.2: never a spelling derived again), then
    /// the directory, whose links are removed as links: nothing they point to is touched. A
    /// marker is trusted only when it names this account and its provider: a marker copied from
    /// another account's profile names that account's spelling, whose Keychain item must never
    /// be deleted here. With no trusted marker, the items for the profile's current canonical
    /// spelling are deleted instead, and a warning says an item under an older spelling may
    /// remain (Decision 12). If that spelling cannot be derived because the path does not
    /// resolve (a dangling link), there is no item to name: the delete is skipped with the same
    /// warning, and the path itself is removed as a link, so a stray path never leaves `remove`
    /// unable to finish once the vault is gone. Any other failure stops before the directory
    /// goes. The items are found by the recorded spelling, the files by the profile's actual
    /// directory (Decision 19).
    pub(crate) fn remove_profile(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<(), EngineError> {
        let profile = profile_path(&self.env, &row.id);
        let meta = match fs::symlink_metadata(&profile) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let current_spelling = |why: String| -> Option<String> {
            match canonical_profile_path(&profile) {
                Ok(canonical) => {
                    tracing::warn!(
                        position = row.position,
                        account = %row.id,
                        "the session profile's marker could not be read ({why}); deleting its Keychain item under its current spelling, so an item under an older spelling may remain"
                    );
                    Some(p.profile_spelling(&canonical))
                }
                Err(e) => {
                    tracing::warn!(
                        position = row.position,
                        account = %row.id,
                        "the session profile's marker could not be read ({why}) and its path does not resolve ({e}); skipping its Keychain item, so an item under an older spelling may remain"
                    );
                    None
                }
            }
        };
        let spelling = match ProfileMarker::read(&profile) {
            Read::Present(marker)
                if marker.account_id == row.id && marker.provider == row.provider =>
            {
                Some(marker.config_dir)
            }
            Read::Present(_) => current_spelling("it names another account".into()),
            Read::Absent => current_spelling("it has none".into()),
            Read::Unreadable(e) => current_spelling(e.to_string()),
        };
        if let Some(spelling) = spelling {
            p.delete_profile_credential(&self.env, &profile, &spelling)?;
        }
        // `remove_dir_all` removes a symlink inside the profile as a link, never following it.
        if meta.is_dir() {
            fs::remove_dir_all(&profile)?;
        } else {
            fs::remove_file(&profile)?;
        }
        Ok(())
    }

    /// The provider's next position (1 with no store yet), and — when a target position was
    /// given — validates that it falls within the allowed bound (§5: never creates anything
    /// when there is no store). `add_live` and `add_token` call this before the mutation
    /// lock is taken, and each re-checks the authoritative value under the lock via
    /// `prepare`. `move_to` calls this after the lock is already held, where this call's own
    /// check is the authoritative one.
    fn next_position_precheck(
        &self,
        provider: &ProviderId,
        position: Option<u32>,
    ) -> Result<u32, EngineError> {
        let next = match self.existing_store()? {
            Some(s) => s.next_position(provider)?,
            None => 1,
        };
        if let Some(pos) = position {
            check_position(pos, next.saturating_sub(1))?;
        }
        Ok(next)
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
        claimed_uuid: Option<&str>,
        position: Option<u32>,
        alias: Option<&str>,
        yes: bool,
    ) -> Result<Prepared, EngineError> {
        let existing = store.find_by_identity_key(provider, p.identity_key(identity).as_str())?;
        check_identity_conflict(existing.as_ref(), claimed_uuid, &identity.label)?;
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
                    return Err(alias_taken(StoreError::AliasTaken(a.to_owned())));
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
    /// occupant it displaces is removed, so a failure never loses the occupant. `source` is the
    /// evidence a replacement records for the default home (§12.5).
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
        source: LoginSource,
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
                // write, the next lock holder installs it (§12.5). A secret with no
                // fingerprint is refused outright rather than recorded as an empty one
                // (Task 18's review, item 7): every caller already guarantees a fingerprint
                // exists by this point, so this is defensive, not a path any test reaches.
                let new_fp = p
                    .fingerprint(secret)
                    .ok_or_else(|| {
                        EngineError::InvalidInput(
                            "the credential has no fingerprint to record".into(),
                        )
                    })?
                    .as_str()
                    .to_owned();
                let meta = LoginMeta {
                    identity_key: key.as_str(),
                    identity,
                    kind,
                    login_expires_at: p.login_expires_at(secret),
                    from_live: source.from_live,
                };
                // §12.5 step 1, with the default home's evidence in the same transaction.
                store.begin_replacement(&row.id, &new_fp, &meta, source.live_names_account)?;
                if let Err(e) = self.vault.store(lock_for(&row.id), secret, &fp) {
                    // The write never landed: reconcile in process rather than leaving the
                    // marker dangling for the next lock holder to find (Task 18's review,
                    // item 6). `reconcile_replacement` reads the vault fresh, sees it still
                    // holds the old generation, and rolls the marker back; a failure here is
                    // swallowed since the next lock acquisition retries it regardless.
                    let _ = self.reconcile_replacement(lock_for(&row.id));
                    return Err(e.into());
                }
                store.finish_replacement(&row.id, self.now_ms())?;
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
                    // The write may have landed and failed only its read-back. A new account
                    // has no earlier generation to keep, so whatever it left goes with the row:
                    // no secret outlives its account (§5, L444).
                    if let Err(cleanup) = self.vault.delete(lock_for(&prep.id)) {
                        tracing::error!(
                            account = %prep.id,
                            "could not remove the vault entry of an account that was never added: {cleanup}"
                        );
                    }
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
        // Validated before anything exists (§5); `prepare` re-checks under the lock.
        self.next_position_precheck(&opts.provider, opts.position)?;
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
        // §7.6: an oracle answer with no non-empty uuid of its own is treated as unresolved,
        // exactly like no answer at all (Task 18's review, item 3) — never compared by email.
        let mut notices = Vec::new();
        let resolved = oracle_resolved(self.oracle.resolve(p.as_ref(), &cred));
        match &resolved {
            Some(owner) if !same_owner(owner, &identity) => {
                return Err(EngineError::OwnerMismatch {
                    expected: identity.label,
                    found: owner.label.clone(),
                });
            }
            Some(_) => {}
            None => notices.push(format!(
                "could not verify that the live credential belongs to {}",
                identity.label
            )),
        }
        // The most authoritative uuid known for this login: the oracle's, when it resolved
        // one, else the self-reported one — so a self-reported identity with no uuid of its
        // own still benefits from an oracle-confirmed one when checking for a conflict with
        // a stored account (Task 18's review, item 3).
        let claimed_uuid = resolved
            .as_ref()
            .and_then(|o| o.account_uuid.as_deref())
            .or(identity.account_uuid.as_deref());
        let kind = p.classify(cred.bytes());
        crate::hooks::point(self, "add-verified")?;
        // 4. Write, under the mutation lock, the account locks and then CC's live locks.
        let guard = self.guard_or_refuse(&opts.provider)?;
        let store = self.store()?;
        let prep = self.prepare(
            &store,
            p.as_ref(),
            &opts.provider,
            &identity,
            claimed_uuid,
            opts.position,
            alias.as_deref(),
            opts.yes,
        )?;
        let accounts = self.lock_prepared(&prep)?;
        // Reconciling a pending replacement under the account lock just taken (§12.5) may
        // have installed a uuid the pre-lock check in `prepare` never saw: re-run it now
        // that the row is authoritative (Task 18's review, item 1).
        check_identity_conflict(
            store.account(&prep.id)?.as_ref(),
            claimed_uuid,
            &identity.label,
        )?;
        // §10.3 Guard: replacing an occupant removes it, profile and all. Its lock is held now,
        // so no session can start on it before the remove.
        if let Some(occupant) = &prep.occupant {
            self.refuse_destroying(p.as_ref(), occupant)?;
        }
        let live_locks = p.lock_live(&self.env, &guard)?;
        // 3.3 The capture must be the login verified above, on both auth axes: a switch or a
        // recovery may have moved it while this command waited for the locks.
        // The complete identity is compared, not just its key: an `accountUuid` or any other
        // `oauthAccount` field that changed means this is not the login that was verified. An
        // identity that becomes unreadable is reported as such, not collapsed into `LiveMoved`
        // (Task 18's review, item 9): only its absence — the login disappearing outright —
        // reads as "moved".
        let now_identity = match p.live_identity(&self.env) {
            Read::Present(i) => Some(i),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
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
            // `add`'s login is the live one, verified just above under the live locks.
            LoginSource {
                from_live: true,
                live_names_account: true,
            },
        )?;
        drop(live_locks);
        // §10.1: a new account's activation, in a write of its own (no replacement can
        // stale-mark an account that did not exist). A replacement's last transaction has
        // already recorded this same epoch.
        store.set_active(&opts.provider, Some(&account.id), Some(account.login_epoch))?;
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
        let prefix = p.kind_traits(&kind).default_email_prefix;
        // Refused before anything exists (§5): the lock-held branch below cannot need it.
        if opts.email.is_none() && prefix.is_none() {
            return Err(no_default_email(&kind));
        }
        self.next_position_precheck(&opts.provider, opts.position)?;
        if let Some(email) = opts.email.as_deref().filter(|e| !is_valid_email(e)) {
            return Err(EngineError::InvalidInput(format!(
                "{email:?} is not a valid email address"
            )));
        }
        let _guard = self.guard_or_refuse(&opts.provider)?;
        let store = self.store()?;
        let identity = match (&opts.email, prefix) {
            (Some(email), _) => p.token_identity(email),
            // Under the lock the position is authoritative, so a default email follows it.
            (None, Some(prefix)) => {
                let position = match opts.position {
                    Some(pos) => pos,
                    None => store.next_position(&opts.provider)?,
                };
                unused_token_identity(&store, p.as_ref(), &opts.provider, prefix, position)?
            }
            (None, None) => return Err(no_default_email(&kind)),
        };
        let claimed_uuid = identity.account_uuid.as_deref();
        let prep = self.prepare(
            &store,
            p.as_ref(),
            &opts.provider,
            &identity,
            claimed_uuid,
            opts.position,
            alias.as_deref(),
            opts.yes,
        )?;
        let accounts = self.lock_prepared(&prep)?;
        // The different-kind collision (§10.2) is decided only now: taking the account lock
        // may have finished a pending replacement and changed the account's kind. The uuid
        // conflict check is re-run here too, for the same reason (Task 18's review, item 1) —
        // a no-op today since a token identity never claims a uuid, but it keeps the two
        // rechecks in one place rather than only one of them surviving the next change.
        let current = store.account(&prep.id)?;
        check_identity_conflict(current.as_ref(), claimed_uuid, &identity.label)?;
        // §10.3 Guard, as in `add_live`.
        if let Some(occupant) = &prep.occupant {
            self.refuse_destroying(p.as_ref(), occupant)?;
        }
        if let Some(existing) = &current {
            if existing.kind != kind {
                return Err(EngineError::InvalidInput(format!(
                    "{} is already stored as a {} account",
                    identity.label, existing.kind
                )));
            }
        }
        // §12.5: whether the live identity names this account, read now that its lock is
        // held: a file read, with no Keychain. One that cannot be read leaves the evidence
        // undecidable, and either guess can cost a replacement its protection, so nothing is
        // written (B.1).
        let live_names_account = match p.live_identity(&self.env) {
            Read::Present(live) => p.identity_key(&live) == p.identity_key(&identity),
            Read::Absent => false,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
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
            LoginSource {
                from_live: false,
                live_names_account,
            },
        )?;
        Ok(AddOutcome {
            account,
            created,
            notices: vec![],
        })
    }

    /// Never creates the store (§5): a missing store means a missing account, not an empty
    /// one to open.
    fn managed_row(&self, id: &AccountId) -> Result<AccountRow, EngineError> {
        let row = match self.existing_store()? {
            Some(s) => s.account(id)?,
            None => None,
        };
        row.ok_or_else(|| EngineError::NoSuchAccount(id.to_string()))
    }

    /// §10.3. The live login is never touched.
    pub fn remove(&self, id: &AccountId) -> Result<AccountRow, EngineError> {
        self.refuse_inside_run_shell()?;
        let provider = self.managed_row(id)?.provider;
        self.settle_or_refuse(&provider)?;
        let _guard = self.guard_or_refuse(&provider)?;
        let row = self.managed_row(id)?;
        // Not `lock_account`: its strict reconciliation refuses a replacement that cannot be
        // installed, which `remove_locked` deletes instead (§12.5).
        let lock = AccountLock::acquire(&self.env, id, AccountLock::WAIT)?;
        // §10.3 Guard: under the mutation lock and the account lock, no session can start
        // before the remove is done (§12.5).
        let p = self.provider(&row.provider)?;
        self.refuse_destroying(p.as_ref(), &row)?;
        self.remove_locked(&row, &lock)?;
        Ok(row)
    }

    /// Changes only store metadata (§9.6, amended): proceeds even while a switch for this
    /// account's provider is undecidable, unlike `remove`. The existence check runs before
    /// the mutation lock is taken (§5: a missing account must not create `.mutation.lock`
    /// or the store), then is re-read under the lock, as `remove` already does. It never asks
    /// the oracle (§7.6).
    pub fn set_alias(
        &self,
        id: &AccountId,
        alias: Option<&str>,
    ) -> Result<AccountRow, EngineError> {
        self.refuse_inside_run_shell()?;
        let alias = alias_arg(alias)?;
        self.managed_row(id)?;
        let _guard = self.metadata_guard()?;
        self.managed_row(id)?;
        self.store()?
            .set_alias(id, alias.as_deref())
            .map_err(alias_taken)?;
        self.managed_row(id)
    }

    /// Changes only store metadata (§9.6, amended): proceeds even while a switch for this
    /// account's provider is undecidable, unlike `remove`. The existence check runs before
    /// the mutation lock is taken (§5: a missing account must not create `.mutation.lock`
    /// or the store), then is re-read under the lock, as `remove` already does. It never asks
    /// the oracle (§7.6).
    pub fn set_disabled(&self, id: &AccountId, disabled: bool) -> Result<AccountRow, EngineError> {
        self.refuse_inside_run_shell()?;
        self.managed_row(id)?;
        let _guard = self.metadata_guard()?;
        self.managed_row(id)?;
        self.store()?.set_disabled(id, disabled)?;
        self.managed_row(id)
    }

    /// Reorders only; if the position is taken, the two accounts swap. Changes only store
    /// metadata (§9.6, amended): proceeds even while a switch for this account's provider is
    /// undecidable, unlike `remove`. The existence check runs before the mutation lock is
    /// taken (§5: a missing account must not create `.mutation.lock` or the store), then is
    /// re-read under the lock, as `remove` already does. It never asks the oracle (§7.6).
    pub fn move_to(&self, id: &AccountId, position: u32) -> Result<AccountRow, EngineError> {
        self.refuse_inside_run_shell()?;
        self.managed_row(id)?;
        let _guard = self.metadata_guard()?;
        let row = self.managed_row(id)?;
        // Reuses the same check `add_live`/`add_token` share, instead of repeating
        // `check_position(position, store.next_position(&row.provider)?.saturating_sub(1))`
        // a third time.
        self.next_position_precheck(&row.provider, Some(position))?;
        self.store()?.move_to(id, position)?;
        self.managed_row(id)
    }
}
