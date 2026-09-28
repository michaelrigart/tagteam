use serde_json::Value;
use tagteam_core::OracleVerdict;
use tagteam_provider::{Credential, LiveAuth, LiveLocks, MutationGuard, Provider};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::oracle::verdict;
use crate::store::{AccountRow, EventRow, JournalRow, Store};
use crate::switch::{Axis, OracleHint, answer_for, refuse_unsafe_live_reads};

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
    /// its row refuses through `refuse_if_interrupted` instead. Never creates the store.
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
        let _accounts = self.lock_accounts(&ids)?;
        let locks = p.lock_live(&self.env, guard)?;
        let live = p.read_live_auth(&self.env);
        match self.direction(p, &store, row, &live, hints)? {
            Direction::Forward(fp) => self.finish_forward(p, &store, row, &locks, &live, &fp),
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

    /// The live identity is `expected` and every auth axis names the account on `own` with
    /// generation `fp`, on a fresh re-read, while CC's locks are still ours.
    fn surfaces_agree(
        &self,
        p: &dyn Provider,
        locks: &LiveLocks<'_>,
        own: Axis,
        fp: &str,
        expected: Option<&Value>,
    ) -> bool {
        let identity = p.live_identity(&self.env).present().map(|i| i.raw);
        locks.check_owned().is_ok()
            && identity.as_ref() == expected
            && axes_coherent(p, &p.read_live_auth(&self.env), own, fp)
    }

    /// The switch landed: clear the other auth axis (§9.4 step 7, displacing a stray secret
    /// there first), splice the target's `oauthAccount`, and commit (step 9).
    fn finish_forward(
        &self,
        p: &dyn Provider,
        store: &Store,
        row: &JournalRow,
        locks: &LiveLocks<'_>,
        live: &LiveAuth,
        established: &str,
    ) -> Result<(), EngineError> {
        let to = store
            .account(&row.to_id)?
            .ok_or_else(|| EngineError::NoSuchAccount(row.to_id.to_string()))?;
        let own = Axis::of(&to.kind);
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
        // Step 7's off-axis rule covers only the axis the outgoing account is not on. After a
        // cross-axis switch the axis being cleared may hold the outgoing generation the row
        // journaled, which step 4 already settled with its vault: clearing it loses nothing.
        // Anything else there, a generation CC rotated since the crash included, is saved.
        let journaled_outgoing = row
            .from_fp
            .as_deref()
            .is_some_and(|fp| holds(p, live, own.other(), fp));
        if !journaled_outgoing {
            self.displace_unless_target(
                p,
                &row.provider,
                own.other(),
                live,
                &target_secret,
                live_identity.as_ref(),
                &mut warnings,
            )?;
        }
        for w in &warnings {
            tracing::warn!(provider = %row.provider, "recovering an interrupted switch: {w}");
        }
        // The undos are dropped, never run: undoing would write an old credential back.
        p.clear_other_axis(&self.env, locks, &to.kind)?;
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
        let own = from.as_ref().map_or(Axis::Entry, |r| Axis::of(&r.kind));
        if !holds(p, live, own, established) {
            return Ok(());
        }
        let expected = row.from_identity.as_ref();
        if p.live_identity(&self.env).present().map(|i| i.raw).as_ref() != expected {
            let identity = expected.map(|v| p.parse_identity(v)).transpose()?;
            p.write_identity(&self.env, locks, identity.as_ref())?;
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
