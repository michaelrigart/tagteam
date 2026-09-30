//! §7.4: one strike quarantines an account, bound to the fingerprint that was sent.

use serde_json::json;
use tagteam_provider::DeadReason;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{AccountRow, EventRow};

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
    #[cfg_attr(not(test), allow(dead_code))]
    fn quarantine_event(
        &self,
        row: &AccountRow,
        kind: &str,
        reason: Option<&str>,
    ) -> Result<(), EngineError> {
        self.store()?.insert_event(&EventRow {
            at: self.now_ms(),
            provider: row.provider.clone(),
            kind: kind.into(),
            from_id: None,
            to_id: Some(row.id.clone()),
            trigger: None,
            source: "cli".into(),
            detail: reason.map(|r| json!({"reason": r})),
        })?;
        Ok(())
    }

    /// Sets the quarantine bound to `fp`, the fingerprint that was actually sent, and records a
    /// `quarantine` event (§7.4). The log names the position and ID only (§4.4).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn quarantine(
        &self,
        row: &AccountRow,
        reason: QuarantineReason,
        fp: &str,
    ) -> Result<(), EngineError> {
        self.store()?
            .set_quarantine(&row.id, reason.as_str(), fp, self.now_ms())?;
        self.quarantine_event(row, "quarantine", Some(reason.as_str()))?;
        tracing::warn!(
            position = row.position,
            account = %row.id,
            reason = reason.as_str(),
            "quarantined: the account needs a new login"
        );
        Ok(())
    }

    /// Clears the quarantine and records `unquarantine`; `false` when there was none.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn unquarantine(&self, row: &AccountRow) -> Result<bool, EngineError> {
        let cleared = self.store()?.clear_quarantine(&row.id)?;
        if cleared {
            self.quarantine_event(row, "unquarantine", None)?;
        }
        Ok(cleared)
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
