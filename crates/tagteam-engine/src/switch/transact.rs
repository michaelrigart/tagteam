use tagteam_core::{
    AccountId, OutgoingAction, OutgoingClass, OutgoingFacts, ProviderId, decide_outgoing,
};
use tagteam_provider::{
    BeforeFallback, Identity, LiveAuth, LiveChange, LiveLocks, ProcessStamp, Provider,
    ProviderError, Read, ReadError, SecretStore, StoredLogin,
};

use crate::account_lock::AccountLock;
use crate::displace::displace;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::oracle::verdict;
use crate::provenance::ProfileCheck;
use crate::store::{AccountRow, AutoRecord, CREDENTIALS_REPLACED, EventRow, JournalRow, Store};

use super::plan::Plan;
use super::rollback::Rollback;
use super::support::{
    Axis, Held, OracleHint, answer_for, attributed, needs_relogin, profile_conflict,
    refuse_unreadable, refuse_unsafe_live_reads, same_generation, session_owned, unless_forced,
    works_until_expiry,
};
use super::{
    SwitchOutcome, SwitchReason, SwitchRequest, SwitchTarget, activation_hint, already_active, noop,
};

/// What a row says about a login, for noticing that it changed: a finished replacement
/// (§12.5) keeps the ID but may change the kind or the identity.
fn login_of(row: Option<&AccountRow>) -> Option<(&AccountId, &str, &str)> {
    row.map(|r| (&r.id, r.kind.as_str(), r.identity_key.as_str()))
}

/// The live login and the rows the plan was made for, re-read under every lock.
pub(super) struct Locked {
    live_identity: Option<Identity>,
    target: AccountRow,
    outgoing: Option<AccountRow>,
    /// What the locked re-read decided to tell the user (§9.4 step 1).
    warnings: Vec<String>,
}

/// What `rederive` found under the locks.
pub(super) enum Rederived {
    /// Nothing the plan rests on moved.
    Go(Locked),
    /// Something moved: plan again.
    Replan,
    /// Another process already did this command's work.
    Done(SwitchOutcome),
}

impl Engine {
    /// §9.4 step 1: the live account, the target and the self-switch decision, all re-read
    /// under the locks. `Replan` when any of them moved, or the account locks held are no
    /// longer the outgoing account's; planning again then reaches the right outcome.
    pub(super) fn rederive(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
        plan: &Plan,
        outgoing: Option<&AccountRow>,
    ) -> Result<Rederived, EngineError> {
        let (live_identity, again) = self.live_row(p, store, &req.provider)?;
        // §9.4 step 1: an automatic switch re-checks its preconditions here, before anything is
        // written. A refusal ends the switch; it never falls through to a plan made elsewhere.
        if let Some(refused) = self.auto_refusal(p, store, req, again.as_ref())? {
            return Ok(Rederived::Done(refused));
        }
        // A login that became unmanaged is §9.2's no-op; a target removed meanwhile is
        // replaced (rotation) or reported (direct).
        if live_identity.is_some() && again.is_none() && !req.force {
            return Ok(Rederived::Replan);
        }
        let Some(target) = store.account(&plan.target.id)? else {
            return Ok(Rederived::Replan);
        };
        let self_switch = again.as_ref().is_some_and(|r| r.id == target.id);
        // Another process landed exactly this plan's pick while this one waited for the mutation
        // lock (a double-fired `switch`). That was this command's work; moving on from there would
        // switch twice. A direct target needs no such rule: planning again finds the
        // self-switch no-op.
        if req.target.chosen() && self_switch && !plan.self_switch {
            return Ok(Rederived::Done(noop(
                plan.strategy,
                SwitchReason::AlreadyActive,
                already_active(&target),
                again,
                None,
            )));
        }
        // §9.4 step 1: the target is not session-owned. Launch reservations are written under
        // `MutationGuard` and refreshes under the account lock, both held here, so the answer
        // stands until the locks are released. A direct target is refused (§9.2); a rotation or
        // a usage strategy plans again, and its walk skips the account (§9.3). An automatic
        // switch never gets here with one: `auto_refusal` above answers it `not-candidate`.
        let state = self.session_state(p, &target)?;
        if state.owned() {
            return if req.target.chosen() {
                Ok(Rederived::Replan)
            } else {
                Err(session_owned(p, &target, &state))
            };
        }
        // The rotation decision, recomputed from the store and each candidate's session state
        // (§9.2, §9.3: every candidate list skips session-owned accounts), including the
        // fewer-than-two case; no vault but the target's is read here. Its anchor is `again`,
        // which is `outgoing` when anything proceeds.
        // Only the target's vault is read here (§9.3): if it emptied while this command waited,
        // a rotation plans again and rotates on, and a direct switch reports it.
        if !self.has_login(&target)? {
            return Ok(Rederived::Replan);
        }
        // §9.4 step 1 (amended): a refresh that finished while this switch waited for the
        // target's account lock may have quarantined it (§7.4 `successor_lost`). This is
        // decided from the row just read, never from what the plan saw: a plan made on an
        // earlier attempt may already hold the quarantine, yet nothing else applies §7.2's
        // quarantined-target rule under the locks. A rotation plans again, and the walk skips
        // the row. A direct target is refused when its access token is due, and otherwise
        // activated with the warning (unless freshening already gave it). An unforced
        // self-switch activates the live generation, which no refresh has spent, so the rule
        // does not apply to it (§7.2: only CC refreshes a live token); a forced one re-activates
        // the vault's generation (§9.2), so it does.
        let mut warnings = Vec::new();
        if target.quarantine_reason.is_some() {
            if req.target.chosen() {
                return Ok(Rederived::Replan);
            }
            if (!self_switch || req.force) && p.kind_traits(&target.kind).refreshable {
                let vault = self.read_target(&target)?;
                if self.due(p, &vault) {
                    return Err(needs_relogin(p, &target));
                }
                let warning = works_until_expiry(&target);
                if !plan.warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
        }
        let same_pick = match req.target {
            SwitchTarget::Account(_) => true,
            SwitchTarget::Rotation => self.rotation_pick_stands(p, store, plan, again.as_ref())?,
            // Decision 11: no network under the locks, so the ranking is not recomputed. The
            // pick stands while it is still a candidate and the live login is the account it
            // was ranked against; otherwise planning again ranks from the store's readings.
            SwitchTarget::Usage { .. } => {
                self.switch_candidate(p, &target)?
                    && again.as_ref().map(|r| &r.id) == plan.anchor.as_ref()
            }
        };
        // Account-lock acquisition may have finished a pending replacement (§12.5), changing
        // the outgoing account's kind or identity: compare the rows, not just their IDs.
        let unchanged = login_of(again.as_ref()) == login_of(outgoing)
            && target.kind == plan.target.kind
            && target.identity_key == plan.target.identity_key
            && self_switch == plan.self_switch
            && same_pick;
        Ok(if unchanged {
            Rederived::Go(Locked {
                live_identity,
                target,
                outgoing: again,
                warnings,
            })
        } else {
            Rederived::Replan
        })
    }

    pub(super) fn read_target(&self, target: &AccountRow) -> Result<Vec<u8>, EngineError> {
        match self.vault.read(&target.id) {
            Read::Present(b) if !b.is_empty() => Ok(b),
            Read::Unreadable(source) => Err(EngineError::UnreadableAccount {
                position: target.position,
                label: target.label.clone(),
                source,
            }),
            _ => Err(EngineError::InvalidInput(format!(
                "{} has no stored credential",
                target.label
            ))),
        }
    }

    /// §9.2 and §12.5 "Lazy capture", under the target's account lock and after its rescues
    /// are settled (§6.2). A quiescent profile that rotated since its seed is adopted into the
    /// vault, so the activation that follows reads its generation. A conflict refuses, since the
    /// vault's generation may be consumed. A profile that cannot be read refuses as an
    /// unreadable vault does, naming the account (Decision 9).
    pub(super) fn settle_profile(
        &self,
        p: &dyn Provider,
        target: &AccountRow,
        lock: &AccountLock,
    ) -> Result<(), EngineError> {
        let unreadable = |source: ReadError| EngineError::UnreadableAccount {
            position: target.position,
            label: target.label.clone(),
            source,
        };
        match self.apply_provenance(p, target, lock) {
            Ok(ProfileCheck::Conflict) => Err(profile_conflict(target)),
            Ok(ProfileCheck::Unreadable(detail)) => {
                Err(unreadable(ReadError::new("its session profile", detail)))
            }
            Ok(_) => Ok(()),
            Err(EngineError::Unreadable(source)) => Err(unreadable(source)),
            Err(e) => Err(e),
        }
    }

    /// Steps 2–10, under every lock. Everything here uses the rows read under the locks.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn transact(
        &self,
        p: &dyn Provider,
        store: &Store,
        plan: &Plan,
        locked: Locked,
        account_locks: &[AccountLock],
        locks: &LiveLocks<'_>,
        req: &SwitchRequest,
    ) -> Result<SwitchOutcome, EngineError> {
        let Locked {
            live_identity,
            target,
            outgoing,
            warnings: locked_warnings,
        } = locked;
        let provider = &req.provider;
        let target_identity = p.parse_identity(&target.identity_json)?;
        let live = p.read_live_auth(&self.env);
        refuse_unsafe_live_reads(&live)?;
        let managed_key_replaced = matches!(&live.managed_key, Read::Present(k) if !k.is_empty());
        let doomed = p.doomed(&self.env, locks, LiveChange::Write(&target.kind));
        refuse_unreadable(&doomed)?;

        // §6.2 "Pending rescues before activation": a rescue has already spent the vault's
        // generation. It is settled here, under the target's account lock and before anything
        // is written, so neither branch below reads the spent generation (§9.4 steps 2 and 5).
        // A rescue that cannot be read or adopted refuses the switch before its journal row
        // exists, so there is nothing to roll back.
        let target_lock = account_locks
            .iter()
            .find(|l| l.id() == &target.id)
            .expect("the target is locked");
        self.settle_rescues(p, &target, target_lock)?;
        // §9.2 lazy capture, after the rescues and before either branch reads the target's
        // vault: a rotation the profile holds is the generation composed and written below. A
        // conflict or an unreadable profile refuses before the journal row exists, so there is
        // nothing to roll back.
        self.settle_profile(p, &target, target_lock)?;

        let mut warnings = plan.notes.clone();
        warnings.extend(plan.warnings.iter().cloned());
        warnings.extend(locked_warnings);
        // The live login's own secret, the only one the live identity names when a secret is
        // displaced (§6.3, Decision 13): the live secret on a stored account's own axis, or the
        // credential entry's for an unmanaged login, which `oauthAccount` names. Unlike step 6's
        // `from_secret`, it never falls back to the other axis.
        let own_secret = match &outgoing {
            Some(o) => Axis::of(p, &o.kind).live_secret(&live),
            None => Axis::Entry.live_secret(&live),
        };
        // What steps 2 and 4 settle, and the vaults below: step 7's rule never saves it again.
        let mut held = Held::default();
        let target_secret = match outgoing.as_ref().filter(|_| !req.force) {
            // Step 2, the direct branch: displace whatever is live on either axis unless it is
            // identical to the target in vault form (H5.3).
            None => {
                let secret = self.read_target(&target)?;
                let reason = if req.force {
                    "forced-activation"
                } else {
                    "displaced-live-login"
                };
                for axis in Axis::BOTH {
                    if let Some(bytes) = axis
                        .live_secret(&live)
                        .filter(|b| *p.vault_form(b) != *p.vault_form(&secret))
                    {
                        let identity =
                            attributed(live_identity.as_ref(), own_secret.as_deref(), &bytes);
                        let saved = self.displace_live(
                            p,
                            provider,
                            &bytes,
                            reason,
                            identity,
                            &mut warnings,
                        );
                        held.hold(p, &bytes);
                        unless_forced(saved, req.force, &mut warnings)?;
                    }
                }
                secret
            }
            Some(out) => {
                self.settle_outgoing(
                    p,
                    store,
                    out,
                    &live,
                    plan.hint.as_ref(),
                    account_locks,
                    live_identity.as_ref(),
                    req.source,
                    &mut warnings,
                )?;
                // Step 4 settled the outgoing generation: kept, captured or displaced.
                if let Some(bytes) = Axis::of(p, &out.kind).live_secret(&live) {
                    held.hold(p, &bytes);
                }
                // Read only now: settling a self-switch may have captured a newer live
                // generation into this very account, and that is the one to activate.
                self.read_target(&target)?
            }
        };
        // Step 7's rule: every entry the write destroys is saved first, unless a vault of
        // either account, or steps 2 and 4, already hold its generation. A Keychain-refusal
        // fallback reports what it deletes to `before_fallback` too, which finds it held.
        for id in outgoing.iter().map(|o| &o.id).chain([&target.id]) {
            self.hold_vault(p, &mut held, id);
        }
        for entry in &doomed {
            if let Read::Present(bytes) = &entry.bytes {
                self.save_unheld(
                    p,
                    provider,
                    bytes,
                    &mut held,
                    req.force,
                    live_identity.as_ref(),
                    own_secret.as_deref(),
                    &mut warnings,
                )?;
            }
        }

        // Step 6.
        let from_secret = outgoing
            .as_ref()
            .and_then(|o| Axis::of(p, &o.kind).live_secret(&live))
            .or_else(|| Axis::Entry.live_secret(&live));
        // A forced switch may supersede an undecidable row; it is carried along, so a forced
        // switch that never lands puts it back instead of forgetting it (§9.6).
        let prior = if req.force {
            store.journal(provider)?.map(Box::new)
        } else {
            None
        };
        store.insert_journal(&JournalRow {
            provider: provider.clone(),
            holder: ProcessStamp::current()?,
            from_id: outgoing.as_ref().map(|o| o.id.clone()),
            to_id: target.id.clone(),
            from_fp: from_secret
                .as_deref()
                .and_then(|b| p.fingerprint(b))
                .map(|f| f.as_str().to_owned()),
            from_identity: live_identity.as_ref().map(|i| i.raw.clone()),
            to_fp: p
                .fingerprint(&target_secret)
                .map(|f| f.as_str().to_owned())
                .unwrap_or_default(),
            // §9.4 step 6: read under the target's account lock, so it cannot move before
            // the commit; a forward recovery records it (§9.6).
            to_epoch: Some(target.login_epoch),
            started_at: self.now_ms(),
            prior: prior.clone(),
        })?;

        // Steps 7–10.
        let target_login = StoredLogin {
            kind: target.kind.clone(),
            secret: target_secret,
            identity: target_identity,
        };
        let mut tx = Rollback {
            undos: Vec::new(),
            locks,
            store,
            provider,
            from: outgoing.as_ref().map(|o| o.id.clone()),
            to: target.id.clone(),
            prior,
            in_flight: false,
            armed: true,
        };
        let mut before_fallback = |bytes: &[u8]| {
            self.save_unheld(
                p,
                provider,
                bytes,
                &mut held,
                req.force,
                live_identity.as_ref(),
                own_secret.as_deref(),
                &mut warnings,
            )
            .map_err(|e| {
                ProviderError::Invalid(format!(
                    "could not save a credential the Keychain fallback would delete: {e}"
                ))
            })
        };
        let stored_in = match self.apply(
            p,
            &target,
            &target_login,
            outgoing.as_ref(),
            req,
            &mut tx,
            &mut before_fallback,
        ) {
            Ok(stored_in) => {
                tx.disarm();
                stored_in
            }
            Err(cause) => return Err(tx.fail(cause)),
        };

        let (reason, switched) = if plan.self_switch {
            (SwitchReason::Activated, true)
        } else {
            (
                SwitchReason::Switched,
                outgoing.as_ref().map(|o| &o.id) != Some(&target.id),
            )
        };
        let message = format!(
            "Switched to {}",
            target.alias.as_deref().unwrap_or(&target.label)
        );
        let activation_hint = Some(activation_hint(
            p,
            outgoing.as_ref().map(|r| r.kind.as_str()),
            &target.kind,
            &stored_in,
            managed_key_replaced,
        ));
        Ok(SwitchOutcome {
            switched,
            from: outgoing,
            to: Some(target),
            strategy: plan.strategy,
            reason,
            message,
            warnings,
            stored_in: Some(stored_in),
            unmanaged_email: None,
            managed_key_replaced,
            activation_hint,
        })
    }

    /// Step 4: classify the outgoing credential and act on it. A capture's write changes the
    /// vault's fingerprint (it is never `Ours`), so it clears a quarantine (§7.4) and records
    /// the clear with the switch's `source`.
    #[allow(clippy::too_many_arguments)]
    fn settle_outgoing(
        &self,
        p: &dyn Provider,
        store: &Store,
        out: &AccountRow,
        live: &LiveAuth,
        hint: Option<&OracleHint>,
        account_locks: &[AccountLock],
        live_identity: Option<&Identity>,
        source: &str,
        warnings: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        let Some(bytes) = Axis::of(p, &out.kind).live_secret(live) else {
            return Ok(());
        };
        let vault = match self.vault.read(&out.id) {
            Read::Present(v) => Some(v),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let fp_live = p.fingerprint(&bytes);
        let resolved = answer_for(hint, &bytes);
        let ours = vault
            .as_deref()
            .is_some_and(|v| same_generation(p, v, &bytes));
        // §9.4 step 4 `Superseded`: the vault's `.prev` is this generation, so an active-token
        // refresh stored a newer one it could not publish (§7.5). Read only when the live
        // credential is not the vault's own, and tri-state: an unreadable `.prev` may be
        // exactly that generation, and capturing over the newer one would lose it (step 3).
        let superseded = !ours
            && match self.vault.read_prev(&out.id) {
                Read::Present(prev) => same_generation(p, &prev, &bytes),
                Read::Absent => false,
                Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
            };
        let facts = OutgoingFacts {
            bytes_equal_vault: vault.as_deref() == Some(bytes.as_slice()),
            fp_equal_vault: fp_live.is_some()
                && vault.as_deref().and_then(|v| p.fingerprint(v)) == fp_live,
            equals_vault_prev: superseded,
            wiped: p.is_wiped(&bytes),
            tokenless: fp_live.is_none(),
            oracle: verdict(resolved, out),
            lacks_refresh_over_complete: !p.has_refresh_token(&bytes)
                && vault.as_deref().is_some_and(|v| p.has_refresh_token(v)),
            // `out` was read under its account lock, so its epoch cannot move until the commit.
            live_store_stale: store.live_store_stale(out)?,
        };
        let (class, action) = decide_outgoing(&facts);
        match action {
            OutgoingAction::Nothing => {}
            OutgoingAction::CaptureToVault { backfill_uuid } => {
                let lock = account_locks
                    .iter()
                    .find(|l| l.id() == &out.id)
                    .expect("the outgoing account is locked");
                self.vault.store(lock, &bytes, p)?;
                let identity = p.parse_identity(&out.identity_json)?;
                store.update_login(
                    &out.id,
                    &out.identity_key,
                    &identity,
                    &out.kind,
                    p.login_expires_at(&bytes),
                )?;
                self.unquarantine(out, CREDENTIALS_REPLACED, source)?;
                if backfill_uuid {
                    if let Some(uuid) = resolved.and_then(|i| i.account_uuid.as_deref()) {
                        store.backfill_account_uuid(&out.id, uuid)?;
                    }
                }
                if class == OutgoingClass::Unresolved {
                    tracing::warn!(
                        target: "tagteam_engine::switch",
                        position = out.position,
                        account = %out.id,
                        "captured an unverified live credential into the vault; .prev keeps the previous generation"
                    );
                }
            }
            OutgoingAction::Displace => {
                let identity = resolved.or(live_identity).map(|i| &i.raw);
                let id = displace(
                    self,
                    &out.provider,
                    &bytes,
                    fp_live.as_ref(),
                    "displaced-live-login",
                    identity,
                )?;
                warnings.push(if class == OutgoingClass::Foreign {
                    format!(
                        "the live credential did not belong to position {}; it was saved as displaced/{id}.json",
                        out.position
                    )
                } else if facts.live_store_stale {
                    format!(
                        "the live credential predates position {}'s replacement, so it did not replace the stored one; it was saved as displaced/{id}.json",
                        out.position
                    )
                } else {
                    format!(
                        "the live credential had no refresh token, so it did not replace position {}'s stored one; it was saved as displaced/{id}.json",
                        out.position
                    )
                });
            }
        }
        Ok(())
    }

    /// Saves a live secret the switch is about to overwrite or clear (§6.3, B.5), unless it
    /// carries nothing account-scoped: an entry holding only machine-shared keys loses nothing,
    /// since those are carried over. The error is the failed save; whether it aborts is the
    /// caller's call (B.5: it does, except under --force).
    fn displace_live(
        &self,
        p: &dyn Provider,
        provider: &ProviderId,
        bytes: &[u8],
        reason: &str,
        live_identity: Option<&Identity>,
        warnings: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        let Some(fp) = p.fingerprint(bytes) else {
            return Ok(());
        };
        let id = displace(
            self,
            provider,
            bytes,
            Some(&fp),
            reason,
            live_identity.map(|i| &i.raw),
        )?;
        warnings.push(format!(
            "the previous live credential was saved as displaced/{id}.json"
        ));
        Ok(())
    }

    /// Records the generations `id`'s vault holds, current and `.prev`. One that cannot be read
    /// is left out, so what it may hold is saved rather than assumed kept.
    pub(crate) fn hold_vault(&self, p: &dyn Provider, held: &mut Held, id: &AccountId) {
        for bytes in [self.vault.read(id), self.vault.read_prev(id)]
            .into_iter()
            .filter_map(Read::present)
        {
            held.hold(p, &bytes);
        }
    }

    /// §9.4 step 7: saves `bytes`, a live entry a change is about to overwrite or delete, unless
    /// it holds nothing account-scoped or a generation `held` already keeps. Either way the
    /// generation is held from then on, so no entry of it is saved twice. A failed save aborts,
    /// except under `force` (B.5). The row names `live_identity` only when `bytes` is
    /// `own_secret`, the live login's own (§6.3, Decision 13). A secret on the other auth axis
    /// gets no identity.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn save_unheld(
        &self,
        p: &dyn Provider,
        provider: &ProviderId,
        bytes: &[u8],
        held: &mut Held,
        force: bool,
        live_identity: Option<&Identity>,
        own_secret: Option<&[u8]>,
        warnings: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        match p.fingerprint(bytes) {
            Some(fp) if held.insert(fp.as_str()) => {
                let saved = self.displace_live(
                    p,
                    provider,
                    bytes,
                    "displaced-live-login",
                    attributed(live_identity, own_secret, bytes),
                    warnings,
                );
                unless_forced(saved, force, warnings)
            }
            _ => Ok(()),
        }
    }

    /// Steps 7–9, keeping an undo for each write.
    #[allow(clippy::too_many_arguments)]
    fn apply<'a>(
        &self,
        p: &dyn Provider,
        target: &AccountRow,
        target_login: &StoredLogin,
        outgoing: Option<&AccountRow>,
        req: &SwitchRequest,
        tx: &mut Rollback<'a, '_>,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<SecretStore, EngineError> {
        let locks = tx.locks;
        hooks::point(self, "after-journal")?;
        // Steps 7–10 are a critical span (§14.1): the write's storage-write wait is not a
        // cancellation point, and neither is its rollback's.
        let env = self.critical_env();
        let stored_in = tx.write(|| {
            p.write_credential(&env, locks, target_login, before_fallback)
                .map(|w| (w.undo, w.stored_in))
        })?;
        hooks::point(self, "after-credential")?;
        tx.write(|| {
            p.write_identity(&self.env, locks, Some(&target_login.identity))
                .map(|undo| (undo, ()))
        })?;
        hooks::point(self, "after-identity")?;
        // §11.2 step 11: an automatic switch records its departure with the commit; its
        // preconditions have made the outgoing account the one it decided on.
        let record = req
            .auto
            .as_ref()
            .zip(outgoing)
            .map(|(auto, from)| AutoRecord {
                at: self.now_ms().div_euclid(1000),
                from: from.id.clone(),
                to: target.id.clone(),
                departure: auto.departure.clone(),
            });
        tx.store.commit_switch(
            &req.provider,
            &target.id,
            target.login_epoch,
            &EventRow {
                at: self.now_ms(),
                provider: req.provider.clone(),
                kind: "switch".into(),
                from_id: outgoing.map(|o| o.id.clone()),
                to_id: Some(target.id.clone()),
                trigger: Some(
                    req.auto
                        .as_ref()
                        .map_or("manual", |a| a.trigger.as_str())
                        .into(),
                ),
                source: req.source.into(),
                detail: None,
            },
            record.as_ref(),
        )?;
        Ok(stored_in)
    }
}
