//! §7.3, the refresh gate, and the one writer of a new generation it shares with rescue
//! adoption and the switch (§6.2, §7.4).

use tagteam_provider::{Provider, Read};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::AccountRow;

impl Engine {
    /// Writes a new generation of `row`'s login under `lock` (§6.2): `.prev` rotates only when
    /// the lineage fingerprint changes, and the write is verified. Then `login_expires_at` is
    /// recorded, and a quarantine is cleared when the fingerprint changed (§7.4). Every writer
    /// of a received or adopted generation goes through here.
    #[cfg_attr(not(test), allow(dead_code))]
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
