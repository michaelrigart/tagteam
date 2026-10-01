use serde_json::Value;
use tagteam_core::{OracleVerdict, OutgoingAction, OutgoingFacts, decide_outgoing};
use tagteam_provider::{
    Credential, LiveAuth, LiveChange, LiveLocks, LockError, MutationGuard, Provider, ProviderError,
    Read,
};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::oracle::verdict;
use crate::store::{AccountRow, EventRow, JournalRow, Store};
use crate::switch::{
    Axis, Held, OracleHint, answer_for, refuse_unreadable, refuse_unsafe_live_reads,
};

/// Which way an interrupted switch went, as the live credential decides it (§9.6), with the
/// fingerprint of the generation established as the chosen account's.
enum Direction {
    Forward(String),
    Backward(String),
    Undecidable,
}

/// The fingerprint of a live secret, on either auth axis, that is generation `fp` or that the
/// pre-lock oracle attributed to `account` (§9.6). The oracle's answer counts only for the exact
/// bytes it was asked about, and only through `verdict` (§7.6).
fn named(
    p: &dyn Provider,
    live: &LiveAuth,
    hints: &[OracleHint],
    fp: Option<&str>,
    account: Option<&AccountRow>,
) -> Option<String> {
    Axis::BOTH
        .into_iter()
        .filter_map(|axis| axis.live_secret(live))
        .find_map(|bytes| {
            let live_fp = p.fingerprint(&bytes)?;
            let by_fp = fp == Some(live_fp.as_str());
            let by_oracle = account.is_some_and(|a| {
                let resolved = hints.iter().find_map(|h| answer_for(Some(h), &bytes));
                verdict(resolved, a) == OracleVerdict::ThisAccount
            });
            (by_fp || by_oracle).then(|| live_fp.as_str().to_owned())
        })
}

/// Whether `axis` holds a live secret of generation `fp`.
fn holds(p: &dyn Provider, live: &LiveAuth, axis: Axis, fp: &str) -> bool {
    axis.live_secret(live)
        .and_then(|b| p.fingerprint(&b))
        .is_some_and(|f| f.as_str() == fp)
}

/// Every auth surface names one account (§9.6): its own axis holds a fresh secret of the
/// generation `fp` established as its, and the other axis carries no authentication. An entry
/// holding only machine-shared keys has no fingerprint, so it carries none. A read that cannot
/// be trusted (unreadable, degraded or empty) is never coherent.
fn axes_coherent(p: &dyn Provider, live: &LiveAuth, own: Axis, fp: &str) -> bool {
    refuse_unsafe_live_reads(live).is_ok()
        && holds(p, live, own, fp)
        && own
            .other()
            .live_secret(live)
            .and_then(|b| p.fingerprint(&b))
            .is_none()
}

impl Engine {
    /// Journal rows whose holder has died (§12.6): a live holder's switch is still running, and
    /// its row refuses through `guard_or_refuse` instead. Never creates the store.
    pub(crate) fn dead_journals(&self) -> Result<Vec<JournalRow>, EngineError> {
        let Some(store) = self.existing_store()? else {
            return Ok(vec![]);
        };
        Ok(store
            .journals()?
            .into_iter()
            .filter(|j| !j.holder.is_live())
            .collect())
    }

    /// The oracle's answers about the live secrets that `row` does not already name by
    /// fingerprint, asked before the mutation lock (§9.6, §7.6). A read that cannot be trusted
    /// decides nothing, so nothing about it is asked.
    pub(crate) fn recovery_hints(&self, row: &JournalRow) -> Vec<OracleHint> {
        let Ok(p) = self.provider(&row.provider) else {
            return vec![];
        };
        let live = p.read_live_auth(&self.env);
        if refuse_unsafe_live_reads(&live).is_err() {
            return vec![];
        }
        Axis::BOTH
            .into_iter()
            .filter_map(|axis| axis.live_secret(&live))
            .filter(|b| {
                p.fingerprint(b).is_some_and(|f| {
                    f.as_str() != row.to_fp && row.from_fp.as_deref() != Some(f.as_str())
                })
            })
            .map(|bytes| {
                let resolved = self
                    .oracle
                    .resolve(p.as_ref(), &Credential::fresh(bytes.clone()));
                OracleHint { bytes, resolved }
            })
            .collect()
    }

    /// Settles one interrupted switch whose holder died (§9.6), under the same locks as a
    /// switch: the account locks of both accounts it names, then CC's locks. The live
    /// credential decides the direction; the row is cleared only once a fresh re-read of every
    /// surface names one account. Never writes an old credential back. An undecidable row
    /// stays until `switch --force` settles it.
    pub(crate) fn recover_one(
        &self,
        guard: &MutationGuard,
        row: &JournalRow,
        hints: &[OracleHint],
    ) -> Result<(), EngineError> {
        let provider = self.provider(&row.provider)?;
        let p = provider.as_ref();
        let store = self.store()?;
        let mut ids = vec![&row.to_id];
        ids.extend(&row.from_id);
        let accounts = self.lock_accounts(&ids)?;
        // The provider is busy, or a crash left its lock behind: a retry recovers the row.
        let locks = p.lock_live(&self.env, guard).map_err(|e| match e {
            ProviderError::Lock(LockError::Timeout(lock)) => EngineError::RecoveryBlocked {
                provider: row.provider.to_string(),
                app: p.display_name(),
                lock,
            },
            e => e.into(),
        })?;
        let live = p.read_live_auth(&self.env);
        match self.direction(p, &store, row, &live, hints)? {
            Direction::Forward(fp) => {
                self.finish_forward(p, &store, row, &accounts, &locks, &live, hints, &fp)
            }
            Direction::Backward(fp) => self.finish_backward(p, &store, row, &locks, &live, &fp),
            Direction::Undecidable => Ok(()),
        }
    }

    /// §9.6's table, in order: the target's generation, then the outgoing one; anything else,
    /// a read that cannot be trusted included, is undecidable.
    fn direction(
        &self,
        p: &dyn Provider,
        store: &Store,
        row: &JournalRow,
        live: &LiveAuth,
        hints: &[OracleHint],
    ) -> Result<Direction, EngineError> {
        if refuse_unsafe_live_reads(live).is_err() {
            return Ok(Direction::Undecidable);
        }
        let to = store.account(&row.to_id)?;
        if let Some(fp) = named(p, live, hints, Some(&row.to_fp), to.as_ref()) {
            return Ok(Direction::Forward(fp));
        }
        let from = match &row.from_id {
            Some(id) => store.account(id)?,
            None => None,
        };
        Ok(
            match named(p, live, hints, row.from_fp.as_deref(), from.as_ref()) {
                Some(fp) => Direction::Backward(fp),
                None => Direction::Undecidable,
            },
        )
    }

    /// The live identity names the same identity as `expected`, by identity key, and every auth axis names the account on `own` with
    /// generation `fp`, on a fresh re-read, while CC's locks are still ours. An identity that
    /// cannot be read agrees with nothing, not even an expected absence.
    fn surfaces_agree(
        &self,
        p: &dyn Provider,
        locks: &LiveLocks<'_>,
        own: Axis,
        fp: &str,
        expected: Option<&Value>,
    ) -> bool {
        let Ok(identity) = self.read_live_identity(p) else {
            return false;
        };
        // A stored identity that no longer parses agrees with nothing.
        let Ok(expected) = expected.map(|v| p.parse_identity(v)).transpose() else {
            return false;
        };
        locks.check_owned().is_ok()
            && identity.map(|i| p.identity_key(&i)) == expected.map(|i| p.identity_key(&i))
            && axes_coherent(p, &p.read_live_auth(&self.env), own, fp)
    }

    /// The switch landed: clear the other auth axis (§9.4 step 7, saving first whatever the
    /// clear destroys that no vault holds), splice the target's `oauthAccount`, and commit
    /// (step 9).
    #[allow(clippy::too_many_arguments)]
    fn finish_forward(
        &self,
        p: &dyn Provider,
        store: &Store,
        row: &JournalRow,
        accounts: &[AccountLock],
        locks: &LiveLocks<'_>,
        live: &LiveAuth,
        hints: &[OracleHint],
        established: &str,
    ) -> Result<(), EngineError> {
        let to = store
            .account(&row.to_id)?
            .ok_or_else(|| EngineError::NoSuchAccount(row.to_id.to_string()))?;
        let own = Axis::of(p, &to.kind);
        // Nothing can make the surfaces agree unless the target's own axis holds the
        // generation established as its.
        let Some(target_secret) = own
            .live_secret(live)
            .filter(|_| holds(p, live, own, established))
        else {
            return Ok(());
        };
        let identity = p.parse_identity(&to.identity_json)?;
        let live_identity = p.live_identity(&self.env).present();
        let mut warnings = Vec::new();
        // §9.4 step 7's rule, before the other axis is cleared: every entry the clear destroys
        // is kept first, unless its generation is held already. Held are the target's live
        // generation, the vaults of both accounts the row names, and the outgoing generation
        // the row journaled, which step 4 settled before the row was written. A generation CC
        // rotated since the crash is none of these: it is captured when the oracle attributes
        // it to the outgoing account, and saved to `displaced/` otherwise.
        let doomed = p.doomed(&self.env, locks, LiveChange::ClearOther(&to.kind));
        refuse_unreadable(&doomed)?;
        let mut held = Held::default();
        held.hold(p, &target_secret);
        if let Some(fp) = &row.from_fp {
            held.insert(fp);
        }
        for id in row.from_id.iter().chain([&to.id]) {
            self.hold_vault(p, &mut held, id);
        }
        // §9.6 (amended): an entry holding a generation no set above holds is the outgoing
        // account's only when the pre-lock oracle attributed exactly those bytes to it. It is
        // then captured into that account's vault; anything else is saved to `displaced/`.
        let from = match &row.from_id {
            Some(id) => store.account(id)?,
            None => None,
        };
        for entry in &doomed {
            let Read::Present(bytes) = &entry.bytes else {
                continue;
            };
            if let Some(from) = &from {
                if self
                    .capture_rotated_outgoing(p, store, from, bytes, hints, accounts, &mut held)?
                {
                    continue;
                }
            }
            self.save_unheld(
                p,
                &row.provider,
                bytes,
                &mut held,
                false,
                live_identity.as_ref(),
                &mut warnings,
            )?;
        }
        for w in &warnings {
            tracing::warn!(provider = %row.provider, "recovering an interrupted switch: {w}");
        }
        // The undos are dropped, never run: undoing would write an old credential back.
        // Recovery's writes are a critical span (§14.1): their storage-write wait is not a
        // cancellation point.
        p.clear_other_axis(&self.critical_env(), locks, &to.kind)?;
        p.write_identity(&self.env, locks, Some(&identity))?;
        hooks::point(self, "recovery-before-commit")?;
        if !self.surfaces_agree(p, locks, own, established, Some(&to.identity_json)) {
            return Ok(());
        }
        store.commit_switch(
            &row.provider,
            &to.id,
            &EventRow {
                at: self.now_ms(),
                provider: row.provider.clone(),
                kind: "switch-recovered".into(),
                from_id: row.from_id.clone(),
                to_id: Some(to.id.clone()),
                trigger: Some("recovery".into()),
                source: "cli".into(),
                detail: None,
            },
        )?;
        Ok(())
    }

    /// §9.6 (amended): an entry a forward finish is about to clear, holding a generation that
    /// none of the held sets does, is classified as §9.4 step 4 would classify it, but
    /// without step 4's `Unresolved` capture. Recovery can run long after the crash, even after
    /// a re-login, so a live login naming the outgoing account no longer implies the credential
    /// is its. It is captured into the outgoing account's vault only when the pre-lock oracle
    /// resolved exactly these bytes to it. The hints are asked only about fresh live reads
    /// (`Axis::live_secret`), so a degraded read can never be captured, and §6.2's
    /// refresh-token bound applies through `decide_outgoing`. Returns whether the entry was
    /// captured, and so is held now.
    #[allow(clippy::too_many_arguments)]
    fn capture_rotated_outgoing(
        &self,
        p: &dyn Provider,
        store: &Store,
        from: &AccountRow,
        bytes: &[u8],
        hints: &[OracleHint],
        accounts: &[AccountLock],
        held: &mut Held,
    ) -> Result<bool, EngineError> {
        let Some(fp) = p.fingerprint(bytes) else {
            return Ok(false);
        };
        if held.contains(fp.as_str()) {
            return Ok(false);
        }
        let resolved = hints.iter().find_map(|h| answer_for(Some(h), bytes));
        let oracle = verdict(resolved, from);
        if oracle != OracleVerdict::ThisAccount {
            return Ok(false);
        }
        let vault = match self.vault.read(&from.id) {
            Read::Present(v) => Some(v),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let facts = OutgoingFacts {
            bytes_equal_vault: vault.as_deref() == Some(bytes),
            fp_equal_vault: vault.as_deref().and_then(|v| p.fingerprint(v)).as_ref() == Some(&fp),
            // Recovery's held set covers the vault's `.prev` (`hold_vault`), so a superseded
            // generation returned early above and never reaches this classification.
            equals_vault_prev: false,
            wiped: p.is_wiped(bytes),
            tokenless: false,
            oracle,
            lacks_refresh_over_complete: !p.has_refresh_token(bytes)
                && vault.as_deref().is_some_and(|v| p.has_refresh_token(v)),
        };
        let OutgoingAction::CaptureToVault { .. } = decide_outgoing(&facts).1 else {
            return Ok(false);
        };
        let lock = accounts
            .iter()
            .find(|l| l.id() == &from.id)
            .expect("recovery locks both accounts the row names");
        self.persist_generation(p, from, lock, bytes)?;
        if let Some(uuid) = resolved.and_then(|i| i.account_uuid.as_deref()) {
            store.backfill_account_uuid(&from.id, uuid)?;
        }
        held.insert(fp.as_str());
        Ok(true)
    }

    /// The switch never landed, or its credential rollback succeeded: without touching the
    /// credential, splice `from_identity` back if it differs and keep the store's active
    /// account. Then put back the row a forced switch superseded, or delete the row.
    fn finish_backward(
        &self,
        p: &dyn Provider,
        store: &Store,
        row: &JournalRow,
        locks: &LiveLocks<'_>,
        live: &LiveAuth,
        established: &str,
    ) -> Result<(), EngineError> {
        // An unmanaged outgoing login was journaled from the credential entry (§9.4 step 6).
        let from = match &row.from_id {
            Some(id) => store.account(id)?,
            None => None,
        };
        let own = from.as_ref().map_or(Axis::Entry, |r| Axis::of(p, &r.kind));
        if !holds(p, live, own, established) {
            return Ok(());
        }
        // An identity that cannot be read decides nothing: the row is undecidable and stays.
        let Ok(live_identity) = self.read_live_identity(p) else {
            return Ok(());
        };
        // §9.6 (amended): splice the journaled identity back only when the live one names a
        // different identity. CC may have updated other fields of the same identity's object
        // since the crash, and that object is kept.
        let expected = row.from_identity.as_ref();
        let expected_identity = expected.map(|v| p.parse_identity(v)).transpose()?;
        let key = |i: &tagteam_provider::Identity| p.identity_key(i);
        if live_identity.as_ref().map(key) != expected_identity.as_ref().map(key) {
            p.write_identity(&self.env, locks, expected_identity.as_ref())?;
        }
        if !self.surfaces_agree(p, locks, own, established, expected) {
            return Ok(());
        }
        match &row.prior {
            Some(prior) => store.insert_journal(prior)?,
            None => store.delete_journal(&row.provider)?,
        }
        Ok(())
    }
}
