//! §7.3, the refresh gate, and the one writer of a new generation it shares with rescue
//! adoption and the switch (§6.2, §7.4).

use std::fmt;
use std::time::Duration;

use tagteam_core::{AccountId, Fingerprint};
use tagteam_provider::provider::{DeadReason, RefreshResult};
use tagteam_provider::{Credential, Identity, Provider, Read};

use crate::account_lock::AccountLock;
use crate::displace::displace;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::quarantine::QuarantineReason;
use crate::store::{AccountRow, Store};

/// §7.3 step 5: the token request's bound. The account lock is held across it, which is why
/// every other vault writer waits up to 15 s (§6.2).
pub const GATE_TIMEOUT: Duration = Duration::from_secs(10);

/// §7.2: an access token counts as expired this long before its `expiresAt`. Shared with
/// active-token refresh (Task 16).
pub(crate) const EXPIRY_BUFFER_MS: i64 = 5 * 60 * 1000;

/// Who the gate left an account's token to (§7.3 step 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnedBy {
    /// It is the live login: only active-token refresh (§7.5) may refresh it.
    Live,
    /// An unresolved switch names it: until recovery decides, CC may be running on it (§9.6).
    Journal,
    /// A `tagteam run` session owns it (§12.5). Never produced before M4.
    Session,
}

/// What the refresh gate did (§7.3). Credential bytes never reach `Debug`.
pub enum GateOutcome {
    /// Refreshed now; the vault holds these bytes.
    Refreshed(Vec<u8>),
    /// Step 4: another process already refreshed it; the vault holds these bytes.
    AlreadyFresh(Vec<u8>),
    /// Another process holds the account lock (step 1).
    Busy,
    Owned(OwnedBy),
    /// A session profile's provenance conflicts (§12.5). Never produced before M4.
    Conflict,
    /// Quarantined, by this pass or an earlier one (§7.4).
    Dead(QuarantineReason),
    /// The token endpoint refused the request itself (`invalid_client`, or an unknown client
    /// id's `invalid_request_error`), quoting its message: never a strike.
    Systemic(String),
    /// Nothing decisive happened. `rescued` means a successor was received but is only in
    /// `rescue/`: the vault's generation is consumed, and the caller must not activate it.
    Transient {
        kind: String,
        rescued: bool,
    },
    /// A successor was received and neither the vault nor `rescue/` could store it.
    Unpersisted,
}

impl fmt::Debug for GateOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateOutcome::Refreshed(b) => write!(f, "Refreshed(<{} bytes>)", b.len()),
            GateOutcome::AlreadyFresh(b) => write!(f, "AlreadyFresh(<{} bytes>)", b.len()),
            GateOutcome::Busy => f.write_str("Busy"),
            GateOutcome::Owned(by) => write!(f, "Owned({by:?})"),
            GateOutcome::Conflict => f.write_str("Conflict"),
            GateOutcome::Dead(reason) => write!(f, "Dead({reason:?})"),
            GateOutcome::Systemic(m) => write!(f, "Systemic({m:?})"),
            GateOutcome::Transient { kind, rescued } => {
                write!(f, "Transient {{ kind: {kind:?}, rescued: {rescued} }}")
            }
            GateOutcome::Unpersisted => f.write_str("Unpersisted"),
        }
    }
}

fn transient(kind: &str) -> GateOutcome {
    GateOutcome::Transient {
        kind: kind.to_owned(),
        rescued: false,
    }
}

/// §7.2's test for an access token, with a non-numeric `expiresAt` counting as not expired.
/// Shared with active-token refresh (Task 16).
pub(crate) fn expired(p: &dyn Provider, bytes: &[u8], now_ms: i64) -> bool {
    p.access_expires_at(bytes)
        .is_some_and(|at| now_ms + EXPIRY_BUFFER_MS >= at)
}

/// The lineage fingerprint as the store records it; empty for bytes that carry no token.
fn fp_str(p: &dyn Provider, bytes: &[u8]) -> String {
    p.fingerprint(bytes)
        .map(|f| f.as_str().to_owned())
        .unwrap_or_default()
}

/// §7.4 `identity_conflict`: the token response names another account. A uuid is compared
/// only when both sides know one, and an organization only when both are non-empty; either
/// disagreeing alone is a conflict. A successor that conflicts is displaced, never stored
/// (§7.3 step 6).
pub(crate) fn names_another_account(owner: &Identity, row: &AccountRow) -> bool {
    let uuid = match (
        owner.account_uuid.as_deref().filter(|u| !u.is_empty()),
        row.account_uuid.as_deref(),
    ) {
        (Some(theirs), Some(ours)) => theirs != ours,
        _ => false,
    };
    let org =
        !owner.org_uuid.is_empty() && !row.org_uuid.is_empty() && owner.org_uuid != row.org_uuid;
    uuid || org
}

impl Engine {
    /// Writes a new generation of `row`'s login under `lock` (§6.2): `.prev` rotates only when
    /// the lineage fingerprint changes, and the write is verified. Then `login_expires_at` is
    /// recorded, and a quarantine is cleared when the fingerprint changed (§7.4). Every writer
    /// of a received or adopted generation goes through here.
    pub(crate) fn persist_generation(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        bytes: &[u8],
    ) -> Result<(), EngineError> {
        debug_assert_eq!(lock.id(), &row.id, "the caller holds this account's lock");
        let before = match self.vault.read(&row.id) {
            Read::Present(b) => p.fingerprint(&b),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        self.vault.store(lock, bytes, &|b| p.fingerprint(b))?;
        self.store()?
            .set_login_expires_at(&row.id, p.login_expires_at(bytes))?;
        if p.fingerprint(bytes) != before {
            self.unquarantine(row)?;
        }
        Ok(())
    }
}

impl Engine {
    /// The refresh gate (§7.3): the only place a stored refresh token is ever sent. The
    /// account lock is only tried, never waited for, and is held from here until the result
    /// is persisted, across the request: at most one refresh per account is in flight, and a
    /// suspended holder is never preempted. `snapshot` is the vault bytes the caller decided
    /// on; step 4 compares against it.
    pub fn refresh_stored(
        &self,
        p: &dyn Provider,
        id: &AccountId,
        snapshot: &[u8],
    ) -> Result<GateOutcome, EngineError> {
        // 1.
        let Some(lock) = AccountLock::try_acquire(&self.env, id)? else {
            return Ok(GateOutcome::Busy);
        };
        // §6.2 "Pending replacements first".
        self.reconcile_replacement(&lock)?;
        let store = self.store()?;
        let Some(row) = store.account(id)? else {
            return Ok(transient("vault-absent"));
        };
        // §7.4: a quarantine holds only while the vault still holds the generation it is bound
        // to. A vault that has moved on releases it (§11.2 step 1), which also heals
        // `persist_generation`'s window between the vault write and the store update.
        if let Some(reason) = &row.quarantine_reason {
            if !self.quarantine_released(p, &row) {
                return Ok(GateOutcome::Dead(
                    QuarantineReason::parse(reason).unwrap_or(QuarantineReason::InvalidGrant),
                ));
            }
            self.unquarantine(&row)?;
        }
        if !p.kind_traits(&row.kind).refreshable {
            return Ok(transient("not-refreshable"));
        }
        // 2.
        if let Some(by) = self.owner_of(p, &store, &row)? {
            return Ok(GateOutcome::Owned(by));
        }
        // 3. The vault, then any rescue that succeeds it, then the vault again.
        match self.vault.read(id) {
            Read::Present(b) if !b.is_empty() => {}
            Read::Present(_) | Read::Absent => return Ok(transient("vault-absent")),
            Read::Unreadable(_) => return Ok(transient("vault-unreadable")),
        }
        match self.settle_rescues(p, &row, &lock) {
            Ok(()) => {}
            Err(EngineError::RescuePending { .. }) => return Ok(transient("rescue-unreadable")),
            Err(e) => return Err(e),
        }
        let current = match self.vault.read(id) {
            Read::Present(b) if !b.is_empty() => b,
            Read::Present(_) | Read::Absent => return Ok(transient("vault-absent")),
            Read::Unreadable(_) => return Ok(transient("vault-unreadable")),
        };
        // 4.
        let now = self.now_ms();
        if p.access_fingerprint(&current) != p.access_fingerprint(snapshot)
            && !expired(p, &current, now)
        {
            return Ok(GateOutcome::AlreadyFresh(current));
        }
        // 5. Only the account lock is held across the request.
        let sent_fp = fp_str(p, &current);
        let fresh = Credential::fresh(current)
            .into_fresh()
            .expect("a vault read is authoritative, never degraded");
        hooks::point(self, "gate-before-request")?;
        let result = p.refresh(self.http.as_ref(), &fresh, now, GATE_TIMEOUT);
        hooks::point(self, "gate-after-response")?;
        let outcome = match result {
            RefreshResult::Refreshed { successor, owner } => {
                self.persist_refreshed(p, &row, &lock, &sent_fp, successor, owner.as_ref())?
            }
            other => self.verdict(p, &row, &sent_fp, other)?,
        };
        drop(lock);
        Ok(outcome)
    }

    /// Whether `row`'s quarantine no longer binds: the vault is readable and holds a
    /// generation other than the one the quarantine is bound to. Anything less (unreadable,
    /// absent, empty, or an unbound quarantine) leaves it standing.
    fn quarantine_released(&self, p: &dyn Provider, row: &AccountRow) -> bool {
        match self.vault.read(&row.id) {
            Read::Present(b) if !b.is_empty() => row
                .quarantine_fp
                .as_deref()
                .is_some_and(|bound| bound != fp_str(p, &b)),
            _ => false,
        }
    }

    /// §7.3 step 2: whether the account's token is someone else's to refresh.
    fn owner_of(
        &self,
        p: &dyn Provider,
        store: &Store,
        row: &AccountRow,
    ) -> Result<Option<OwnedBy>, EngineError> {
        let live = match p.live_identity(&self.env) {
            Read::Present(i) => p.identity_key(&i).as_str() == row.identity_key,
            Read::Absent => false,
            // It may be this account's login, and the gate never refreshes what might be live.
            Read::Unreadable(_) => true,
        };
        if live {
            return Ok(Some(OwnedBy::Live));
        }
        let journaled = store
            .journals()?
            .iter()
            .any(|j| j.to_id == row.id || j.from_id.as_ref() == Some(&row.id));
        if journaled {
            return Ok(Some(OwnedBy::Journal));
        }
        if self.session_owned(row) {
            return Ok(Some(OwnedBy::Session));
        }
        Ok(None)
    }

    /// §12.5: whether a `tagteam run` session owns the account. Profiles arrive with M4; until
    /// then nothing is session-owned.
    fn session_owned(&self, _row: &AccountRow) -> bool {
        false
    }

    /// Step 7 for every result but a successor.
    fn verdict(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        sent_fp: &str,
        result: RefreshResult,
    ) -> Result<GateOutcome, EngineError> {
        Ok(match result {
            RefreshResult::Refreshed { .. } => unreachable!("handled by the caller"),
            RefreshResult::Dead(DeadReason::InvalidGrant) => {
                // A strike only if the generation sent is still the vault's: if another writer
                // moved the lineage meanwhile, the refusal says nothing about the new one.
                let unchanged = matches!(
                    self.vault.read(&row.id),
                    Read::Present(b) if fp_str(p, &b) == sent_fp
                );
                if !unchanged {
                    return Ok(transient("refresh-failed"));
                }
                self.quarantine(row, QuarantineReason::InvalidGrant, sent_fp)?;
                GateOutcome::Dead(QuarantineReason::InvalidGrant)
            }
            RefreshResult::Dead(DeadReason::NoRefreshToken) => {
                self.quarantine(row, QuarantineReason::NoRefreshToken, sent_fp)?;
                GateOutcome::Dead(QuarantineReason::NoRefreshToken)
            }
            RefreshResult::Systemic(message) => GateOutcome::Systemic(message),
            RefreshResult::Transient(kind) => transient(&kind.token()),
        })
    }

    /// A received successor (§7.3 step 6). One the token endpoint says belongs to another
    /// account (§7.4) is checked first, before anything is stored: it is displaced and the
    /// account quarantined, and it never reaches this account's vault. Task 11 replaces the
    /// rest with the compare-and-swap and rescue of step 6.
    fn persist_refreshed(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        sent_fp: &str,
        successor: Vec<u8>,
        owner: Option<&Identity>,
    ) -> Result<GateOutcome, EngineError> {
        if let Some(owner) = owner.filter(|o| names_another_account(o, row)) {
            return self.displace_foreign(p, row, sent_fp, &successor, owner);
        }
        self.persist_generation(p, row, lock, &successor)?;
        Ok(GateOutcome::Refreshed(successor))
    }

    /// §7.3 step 6 and §7.4: a successor that belongs to another account is displaced, never
    /// written to this account's vault or to `rescue/` (where a later switch could adopt it),
    /// and the account is quarantined, bound to the generation that was sent, which the vault
    /// still holds. If the displacement fails the successor is lost: `Unpersisted`, with the
    /// quarantine still set.
    pub(crate) fn displace_foreign(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        sent_fp: &str,
        successor: &[u8],
        owner: &Identity,
    ) -> Result<GateOutcome, EngineError> {
        let kept = self.keep_foreign(row, successor, p.fingerprint(successor).as_ref(), owner);
        let quarantined = self.quarantine(row, QuarantineReason::IdentityConflict, sent_fp);
        match (kept, quarantined) {
            // A lost successor is reported first (§7.3 step 6).
            (Err(e), _) => {
                tracing::error!(
                    position = row.position,
                    account = %row.id,
                    "a refreshed token that belongs to another account could not be kept: {e}"
                );
                Ok(GateOutcome::Unpersisted)
            }
            (Ok(()), Err(e)) => Err(e),
            (Ok(()), Ok(())) => Ok(GateOutcome::Dead(QuarantineReason::IdentityConflict)),
        }
    }

    /// Writes a successor that belongs to another account to `displaced/` (§6.3, reason
    /// `identity-conflict`), naming the owner the token response gave.
    pub(crate) fn keep_foreign(
        &self,
        row: &AccountRow,
        successor: &[u8],
        fp: Option<&Fingerprint>,
        owner: &Identity,
    ) -> Result<(), EngineError> {
        displace(
            self,
            &row.provider,
            successor,
            fp,
            "identity-conflict",
            Some(&owner.raw),
        )
        .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use crate::testutil::{T, cred};

    fn same_generation_new_access(rt: &str) -> Vec<u8> {
        serde_json::json!({"claudeAiOauth": {"accessToken": "at-other", "refreshToken": rt,
            "expiresAt": 1, "refreshTokenExpiresAt": 7}})
        .to_string()
        .into_bytes()
    }

    #[test]
    fn a_new_generation_rotates_prev_records_expiry_and_clears_the_quarantine() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        t.engine
            .store()
            .unwrap()
            .set_quarantine(&row.id, "invalid_grant", &t.fp(&cred("rt-1", 5)), 1)
            .unwrap();
        let lock = t.lock(&row.id);
        t.engine
            .persist_generation(t.cc.as_ref(), &t.row(&row.id), &lock, &cred("rt-2", 99))
            .unwrap();
        assert_eq!(t.vault_rt(&row.id, false).as_deref(), Some("rt-2"));
        assert_eq!(t.vault_rt(&row.id, true).as_deref(), Some("rt-1"));
        let after = t.row(&row.id);
        assert_eq!(after.login_expires_at, Some(99));
        assert_eq!(
            after.quarantine_reason, None,
            "§7.4: a fingerprint change clears it"
        );
        let kinds: Vec<String> = t
            .engine
            .store()
            .unwrap()
            .events()
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(kinds, ["unquarantine"]);
    }

    #[test]
    fn the_same_generation_keeps_prev_and_the_quarantine() {
        // Review Focus 3's persistence half: a reply without a refresh token keeps the lineage,
        // so `.prev` does not rotate and the strike, bound to that lineage, stands.
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        let fp = t.fp(&cred("rt-1", 5));
        t.engine
            .store()
            .unwrap()
            .set_quarantine(&row.id, "invalid_grant", &fp, 1)
            .unwrap();
        let lock = t.lock(&row.id);
        let next = same_generation_new_access("rt-1");
        t.engine
            .persist_generation(t.cc.as_ref(), &t.row(&row.id), &lock, &next)
            .unwrap();
        assert_eq!(t.kc.get(crate::vault::SERVICE, row.id.as_str()), Some(next));
        assert_eq!(t.vault_rt(&row.id, true), None, "no `.prev`");
        let after = t.row(&row.id);
        assert_eq!(after.quarantine_fp.as_deref(), Some(fp.as_str()));
        assert_eq!(after.login_expires_at, Some(7));
    }

    #[test]
    fn a_failed_vault_write_changes_nothing_in_the_store() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 5));
        t.kc.set_fail_write(crate::vault::SERVICE, true);
        let lock = t.lock(&row.id);
        assert!(
            t.engine
                .persist_generation(t.cc.as_ref(), &row, &lock, &cred("rt-2", 99))
                .is_err()
        );
        t.kc.set_fail_write(crate::vault::SERVICE, false);
        assert_eq!(t.vault_rt(&row.id, false).as_deref(), Some("rt-1"));
        assert_eq!(t.row(&row.id).login_expires_at, None);
    }
}
