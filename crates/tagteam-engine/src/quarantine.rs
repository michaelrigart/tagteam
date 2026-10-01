//! §7.4: one strike quarantines an account, bound to the fingerprint that was sent.

use serde_json::json;
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::{DeadReason, Provenance, Provider, Read};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::refresh::fp_str;
use crate::store::{AccountRow, EventRow};
use crate::switch::Axis;

/// The `quarantine_reason` column's values (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineReason {
    InvalidGrant,
    NoRefreshToken,
    IdentityConflict,
    /// A refresh received a successor and could store it nowhere (§7.3 `Unpersisted`): the
    /// vault's generation is consumed (§7.4).
    SuccessorLost,
}

impl QuarantineReason {
    pub fn as_str(self) -> &'static str {
        match self {
            QuarantineReason::InvalidGrant => "invalid_grant",
            QuarantineReason::NoRefreshToken => "no_refresh_token",
            QuarantineReason::IdentityConflict => "identity_conflict",
            QuarantineReason::SuccessorLost => "successor_lost",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            QuarantineReason::InvalidGrant,
            QuarantineReason::NoRefreshToken,
            QuarantineReason::IdentityConflict,
            QuarantineReason::SuccessorLost,
        ]
        .into_iter()
        .find(|r| r.as_str() == s)
    }
}

impl From<DeadReason> for QuarantineReason {
    fn from(r: DeadReason) -> Self {
        match r {
            DeadReason::InvalidGrant => QuarantineReason::InvalidGrant,
            DeadReason::NoRefreshToken => QuarantineReason::NoRefreshToken,
        }
    }
}

impl Engine {
    fn quarantine_event(
        &self,
        row: &AccountRow,
        kind: &str,
        reason: Option<&str>,
        source: &str,
    ) -> Result<(), EngineError> {
        self.store()?.insert_event(&EventRow {
            at: self.now_ms(),
            provider: row.provider.clone(),
            kind: kind.into(),
            from_id: None,
            to_id: Some(row.id.clone()),
            trigger: None,
            source: source.into(),
            detail: reason.map(|r| json!({"reason": r})),
        })?;
        Ok(())
    }

    /// Sets the quarantine bound to `fp`, the fingerprint that was actually sent, and records a
    /// `quarantine` event (§7.4). The log names the position and ID only (§4.4).
    pub(crate) fn quarantine(
        &self,
        row: &AccountRow,
        reason: QuarantineReason,
        fp: &str,
    ) -> Result<(), EngineError> {
        self.store()?
            .set_quarantine(&row.id, reason.as_str(), fp, self.now_ms())?;
        self.quarantine_event(row, "quarantine", Some(reason.as_str()), "cli")?;
        tracing::warn!(
            position = row.position,
            account = %row.id,
            reason = reason.as_str(),
            "quarantined: the account needs a new login"
        );
        Ok(())
    }

    /// Clears the quarantine and records `unquarantine`; `false` when there was none.
    pub(crate) fn unquarantine(&self, row: &AccountRow) -> Result<bool, EngineError> {
        self.unquarantine_from(row, "cli")
    }

    /// `unquarantine`, with the event's `source` (`cli` or `auto`).
    fn unquarantine_from(&self, row: &AccountRow, source: &str) -> Result<bool, EngineError> {
        let cleared = self.store()?.clear_quarantine(&row.id)?;
        if cleared {
            self.quarantine_event(row, "unquarantine", None, source)?;
        }
        Ok(cleared)
    }

    /// §7.4: whether `row`'s quarantine no longer binds. The vault must hold another
    /// generation than the one the quarantine is bound to, and, when `row` is the live
    /// account, so must the live credential: the active account's quarantine holds while
    /// either copy matches `quarantine_fp`. The refresh gate and `release_unbound_quarantines`
    /// share it.
    pub(crate) fn quarantine_released(&self, p: &dyn Provider, row: &AccountRow) -> bool {
        self.vault_moved_past(p, row) && !self.live_still_bound(p, row)
    }

    /// The vault half: the vault is readable and holds a generation other than the one the
    /// quarantine is bound to. Anything less (unreadable, absent, empty, or a quarantine bound
    /// to nothing) leaves it standing.
    fn vault_moved_past(&self, p: &dyn Provider, row: &AccountRow) -> bool {
        match self.vault.read(&row.id) {
            Read::Present(b) if !b.is_empty() => row
                .quarantine_fp
                .as_deref()
                .is_some_and(|bound| bound != fp_str(p, &b)),
            _ => false,
        }
    }

    /// The live half: whether `row` is the live login and its live credential may still be
    /// the generation the quarantine is bound to. A live identity that cannot be read may be
    /// `row`'s, so the live credential is compared; a live credential that cannot be read, or a
    /// degraded one, may be exactly that generation, so it counts as bound.
    fn live_still_bound(&self, p: &dyn Provider, row: &AccountRow) -> bool {
        let is_live = match p.live_identity(&self.env) {
            Read::Present(i) => p.identity_key(&i).as_str() == row.identity_key,
            Read::Absent => false,
            Read::Unreadable(_) => true,
        };
        if !is_live {
            return false;
        }
        let Some(bound) = row.quarantine_fp.as_deref() else {
            return true;
        };
        let auth = p.read_live_auth(&self.env);
        let live = match Axis::of(p, &row.kind) {
            Axis::Entry => match auth.credential {
                Read::Present(c) if c.provenance() == Provenance::Fresh => Some(c.bytes().to_vec()),
                Read::Absent => None,
                _ => return true,
            },
            Axis::ManagedKey => match auth.managed_key {
                Read::Present(k) => Some(k),
                Read::Absent => None,
                Read::Unreadable(_) => return true,
            },
        };
        live.is_some_and(|bytes| fp_str(p, &bytes) == bound)
    }

    /// §7.4 / Decision 9: clears every quarantine of `provider` that no longer binds
    /// (`quarantine_released`), each under its account lock (try-only; a busy account is
    /// left), and records each release with `source`. Returns the released accounts.
    pub fn release_unbound_quarantines(
        &self,
        provider: &ProviderId,
        source: &'static str,
    ) -> Result<Vec<AccountId>, EngineError> {
        let Some(store) = self.existing_store()? else {
            return Ok(Vec::new());
        };
        let p = self.provider(provider)?;
        let mut released = Vec::new();
        for listed in store.accounts(provider)? {
            if listed.quarantine_reason.is_none() {
                continue;
            }
            // Whoever holds it may be refreshing or writing this very account: it decides, and
            // the next caller looks again.
            let Some(_lock) = AccountLock::try_acquire(&self.env, &listed.id)? else {
                continue;
            };
            // Read again under the lock: the row may have changed since it was listed.
            let Some(row) = store.account(&listed.id)? else {
                continue;
            };
            if row.quarantine_reason.is_none() || !self.quarantine_released(p.as_ref(), &row) {
                continue;
            }
            if self.unquarantine_from(&row, source)? {
                tracing::info!(
                    position = row.position,
                    account = %row.id,
                    "released a quarantine that no longer binds"
                );
                released.push(row.id);
            }
        }
        Ok(released)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{T, cred};

    #[test]
    fn reasons_round_trip_their_column_values() {
        for r in [
            QuarantineReason::InvalidGrant,
            QuarantineReason::NoRefreshToken,
            QuarantineReason::IdentityConflict,
            QuarantineReason::SuccessorLost,
        ] {
            assert_eq!(QuarantineReason::parse(r.as_str()), Some(r));
        }
        assert_eq!(QuarantineReason::InvalidGrant.as_str(), "invalid_grant");
        assert_eq!(
            QuarantineReason::NoRefreshToken.as_str(),
            "no_refresh_token"
        );
        assert_eq!(
            QuarantineReason::IdentityConflict.as_str(),
            "identity_conflict"
        );
        assert_eq!(QuarantineReason::SuccessorLost.as_str(), "successor_lost");
        assert_eq!(QuarantineReason::parse("refresh failed"), None);
        assert_eq!(
            QuarantineReason::from(DeadReason::InvalidGrant),
            QuarantineReason::InvalidGrant
        );
        assert_eq!(
            QuarantineReason::from(DeadReason::NoRefreshToken),
            QuarantineReason::NoRefreshToken
        );
    }

    #[test]
    fn quarantine_and_unquarantine_record_events() {
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-1", 9));
        t.engine
            .quarantine(&row, QuarantineReason::InvalidGrant, "sha256:sent")
            .unwrap();
        let q = t.row(&row.id);
        assert_eq!(
            (q.quarantine_reason.as_deref(), q.quarantine_fp.as_deref()),
            (Some("invalid_grant"), Some("sha256:sent"))
        );
        assert!(t.engine.unquarantine(&q).unwrap());
        assert!(!t.engine.unquarantine(&q).unwrap(), "nothing left to clear");
        let events = t.engine.store().unwrap().events().unwrap();
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, ["quarantine", "unquarantine"]);
        assert_eq!(events[0].to_id.as_ref(), Some(&row.id));
        assert_eq!(events[0].detail, Some(json!({"reason": "invalid_grant"})));
        assert_eq!(events[0].source, "cli");
    }
}
