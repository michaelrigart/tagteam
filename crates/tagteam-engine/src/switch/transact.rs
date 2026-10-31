use tagteam_core::{AccountId, OutgoingAction, OutgoingClass, OutgoingFacts, decide_outgoing};
use tagteam_provider::{
    BeforeFallback, DoomedEntry, Identity, LiveAuth, LiveChange, LiveLocks, MutationGuard,
    ProcessStamp, Provider, Read, ReadError, SecretStore, StoredLogin,
};

use crate::account_lock::AccountLock;
use crate::displace::displace;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::event::EventKind;
use crate::hooks;
use crate::oracle::verdict;
use crate::provenance::ProfileCheck;
use crate::store::{AccountRow, AutoRecord, CREDENTIALS_REPLACED, EventRow, JournalRow, Store};
use crate::vault::VaultMemo;

use super::plan::Plan;
use super::rollback::Rollback;
use super::support::{
    Axis, OracleHint, Settle, answer_for, attributed, needs_relogin, profile_conflict,
    refuse_unreadable, refuse_unsafe_live_reads, same_generation, session_owned, unless_forced,
    works_until_expiry,
};
use super::{
    SwitchCause, SwitchOutcome, SwitchReason, SwitchRequest, SwitchTarget, activation_hint,
    already_active, noop,
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

/// What a switch holds while it runs under every lock.
pub(super) struct LockedSwitch<'a, 'g> {
    pub(super) guard: &'a MutationGuard,
    pub(super) store: &'a Store,
    pub(super) account_locks: &'a [AccountLock],
    pub(super) locks: &'a LiveLocks<'g>,
    pub(super) memo: &'a VaultMemo<'a>,
}

/// The rows `rederive` locked and what `preflight` read under the locks.
struct Preflight {
    target_identity: Identity,
    live: LiveAuth,
    managed_key_replaced: bool,
    doomed: Vec<DoomedEntry>,
    live_identity: Option<Identity>,
    target: AccountRow,
    outgoing: Option<AccountRow>,
    locked_warnings: Vec<String>,
}

/// What `apply` activates: the target and its login, and the outgoing account it names.
struct ApplyStep<'a> {
    target: &'a AccountRow,
    login: &'a StoredLogin,
    outgoing: Option<&'a AccountRow>,
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
    /// longer the outgoing account's; planning again then reaches the right outcome. Its read
    /// of the target's vault enters `memo`, which the rest of the attempt reads through
    /// (review refactor §8.11).
    pub(super) fn rederive(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
        plan: &Plan,
        outgoing: Option<&AccountRow>,
        memo: &VaultMemo<'_>,
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
        if !self.has_login(&target, Some(memo))? {
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
                let vault = self.read_target(&target, Some(memo))?;
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
            // No network under the locks, so the ranking is not recomputed. The
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

    /// The target's vault credential, or why there is none. Pass a `memo` only under the
    /// switch's account locks (review refactor §8.11).
    pub(super) fn read_target(
        &self,
        target: &AccountRow,
        memo: Option<&VaultMemo<'_>>,
    ) -> Result<Vec<u8>, EngineError> {
        match self.read_vault(&target.id, memo) {
            Read::Present(b) if !b.is_empty() => Ok(b),
            Read::Unreadable(source) => Err(EngineError::UnreadableAccount {
                position: target.position,
                label: target.label.clone(),
                source,
            }),
            _ => Err(EngineError::NoStoredCredential(format!(
                "{} has no stored credential",
                target.label
            ))),
        }
    }

    /// §9.2 and §12.5 "Lazy capture", under the target's account lock and after its rescues
    /// are settled (§6.2). A quiescent profile that rotated since its seed is adopted into the
    /// vault, so the activation that follows reads its generation. A conflict refuses, since the
    /// vault's generation may be consumed. A profile that cannot be read refuses as an
    /// unreadable vault does, naming the account, rather than guessing at its credential.
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
    pub(super) fn transact(
        &self,
        p: &dyn Provider,
        plan: &Plan,
        locked: Locked,
        ls: &LockedSwitch<'_, '_>,
        req: &SwitchRequest,
    ) -> Result<SwitchOutcome, EngineError> {
        let provider = &req.provider;
        let mut pf = self.preflight(p, locked, ls)?;
        self.settle_pre(p, ls, &pf.target)?;

        let mut warnings = plan.notes.clone();
        warnings.extend(plan.warnings.iter().cloned());
        warnings.extend(std::mem::take(&mut pf.locked_warnings));
        // The live login's own secret, the only one the live identity names when a secret is
        // displaced (§6.3): the live secret on a stored account's own axis, or the
        // credential entry's for an unmanaged login, which `oauthAccount` names. Unlike step 6's
        // `from_secret`, it never falls back to the other axis.
        let own_secret = match &pf.outgoing {
            Some(o) => Axis::of(p, &o.kind).live_secret(&pf.live),
            None => Axis::Entry.live_secret(&pf.live),
        };
        // What steps 2 and 4 settle, and the vaults below: step 7's rule never saves it again.
        let mut cx = Settle {
            force: req.force,
            live_identity: pf.live_identity.as_ref(),
            own_secret: own_secret.as_deref(),
            memo: Some(ls.memo),
            ..Settle::new(p, provider, warnings)
        };
        let target_secret = self.settle_branch(&mut cx, ls, plan, req, &pf)?;
        self.hold_doomed(&mut cx, &pf)?;

        let prior = self.journal_row(p, ls, req, &pf, &target_secret)?;
        // Steps 7–10.
        let mut tx = Rollback {
            undos: Vec::new(),
            guard: ls.guard,
            locks: ls.locks,
            store: ls.store,
            provider,
            from: pf.outgoing.as_ref().map(|o| o.id.clone()),
            to: pf.target.id.clone(),
            prior,
            in_flight: false,
            armed: true,
        };
        let target_login = StoredLogin {
            kind: pf.target.kind.clone(),
            secret: target_secret,
            identity: pf.target_identity.clone(),
        };
        let mut before_fallback = |bytes: &[u8]| cx.before_fallback(self, bytes);
        let stored_in = match self.apply(
            p,
            ApplyStep {
                target: &pf.target,
                login: &target_login,
                outgoing: pf.outgoing.as_ref(),
            },
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

        let warnings = cx.warnings;
        Ok(self.outcome(p, plan, pf, stored_in, warnings))
    }

    /// The target's identity, the live login and what the write destroys, read under every
    /// lock. Nothing is written, so a refusal here has nothing to roll back.
    fn preflight(
        &self,
        p: &dyn Provider,
        locked: Locked,
        ls: &LockedSwitch<'_, '_>,
    ) -> Result<Preflight, EngineError> {
        let Locked {
            live_identity,
            target,
            outgoing,
            warnings: locked_warnings,
        } = locked;
        let target_identity = p.parse_identity(&target.identity_json)?;
        let live = p.read_live_auth(&self.env);
        refuse_unsafe_live_reads(&live)?;
        let managed_key_replaced = matches!(&live.managed_key, Read::Present(k) if !k.is_empty());
        let doomed = p.doomed(&self.env, ls.locks, LiveChange::Write(&target.kind));
        refuse_unreadable(&doomed)?;
        Ok(Preflight {
            target_identity,
            live,
            managed_key_replaced,
            doomed,
            live_identity,
            target,
            outgoing,
            locked_warnings,
        })
    }

    /// Settles the target's pending rescues and its session profile.
    fn settle_pre(
        &self,
        p: &dyn Provider,
        ls: &LockedSwitch<'_, '_>,
        target: &AccountRow,
    ) -> Result<(), EngineError> {
        // §6.2 "Pending rescues before activation": a rescue has already spent the vault's
        // generation. It is settled here, under the target's account lock and before anything
        // is written, so neither branch below reads the spent generation (§9.4 steps 2 and 5).
        // A rescue that cannot be read or adopted refuses the switch before its journal row
        // exists, so there is nothing to roll back.
        let target_lock = ls
            .account_locks
            .iter()
            .find(|l| l.id() == &target.id)
            .expect("the target is locked");
        self.settle_rescues(p, target, target_lock)?;
        // §9.2 lazy capture, after the rescues and before either branch reads the target's
        // vault: a rotation the profile holds is the generation composed and written below. A
        // conflict or an unreadable profile refuses before the journal row exists, so there is
        // nothing to roll back.
        self.settle_profile(p, target, target_lock)
    }

    /// Steps 2 and 4: settles the live login and returns the target's secret to activate.
    fn settle_branch(
        &self,
        cx: &mut Settle<'_>,
        ls: &LockedSwitch<'_, '_>,
        plan: &Plan,
        req: &SwitchRequest,
        pf: &Preflight,
    ) -> Result<Vec<u8>, EngineError> {
        let p = cx.p;
        let live = &pf.live;
        Ok(match pf.outgoing.as_ref().filter(|_| !req.force) {
            // Step 2, the direct branch: displace whatever is live on either axis unless it is
            // identical to the target in vault form (H5.3).
            None => {
                let secret = self.read_target(&pf.target, Some(ls.memo))?;
                let reason = if req.force {
                    "forced-activation"
                } else {
                    "displaced-live-login"
                };
                for axis in Axis::BOTH {
                    if let Some(bytes) = axis
                        .live_secret(live)
                        .filter(|b| *p.vault_form(b) != *p.vault_form(&secret))
                    {
                        let identity = attributed(cx.live_identity, cx.own_secret, &bytes);
                        let saved = cx.displace_live(self, &bytes, reason, identity);
                        cx.held.hold(p, &bytes);
                        unless_forced(saved, req.force, &mut cx.warnings)?;
                    }
                }
                secret
            }
            Some(out) => {
                self.settle_outgoing(cx, ls, out, live, plan.hint.as_ref())?;
                // Step 4 settled the outgoing generation: kept, captured or displaced.
                if let Some(bytes) = Axis::of(p, &out.kind).live_secret(live) {
                    cx.held.hold(p, &bytes);
                }
                // Read only now: settling a self-switch may have captured a newer live
                // generation into this very account, and that is the one to activate.
                self.read_target(&pf.target, Some(ls.memo))?
            }
        })
    }

    /// Holds the vaults and saves the entries the write destroys.
    fn hold_doomed(&self, cx: &mut Settle<'_>, pf: &Preflight) -> Result<(), EngineError> {
        // Step 7's rule: every entry the write destroys is saved first, unless a vault of
        // either account, or steps 2 and 4, already hold its generation. A Keychain-refusal
        // fallback reports what it deletes to `before_fallback` too, which finds it held.
        for id in pf.outgoing.iter().map(|o| &o.id).chain([&pf.target.id]) {
            cx.hold_vault(self, id);
        }
        for entry in &pf.doomed {
            if let Read::Present(bytes) = &entry.bytes {
                cx.save_unheld(self, bytes)?;
            }
        }
        Ok(())
    }

    /// Step 6: writes the journal row. Returns the row a forced switch superseded, which the
    /// caller's `Rollback` must carry; nothing fallible may sit between the two.
    fn journal_row(
        &self,
        p: &dyn Provider,
        ls: &LockedSwitch<'_, '_>,
        req: &SwitchRequest,
        pf: &Preflight,
        target_secret: &[u8],
    ) -> Result<Option<Box<JournalRow>>, EngineError> {
        let provider = &req.provider;
        // Step 6.
        let from_secret = pf
            .outgoing
            .as_ref()
            .and_then(|o| Axis::of(p, &o.kind).live_secret(&pf.live))
            .or_else(|| Axis::Entry.live_secret(&pf.live));
        // A forced switch may supersede an undecidable row; it is carried along, so a forced
        // switch that never lands puts it back instead of forgetting it (§9.6).
        let prior = if req.force {
            ls.store.journal(provider)?.map(Box::new)
        } else {
            None
        };
        ls.store.insert_journal(
            ls.guard,
            &JournalRow {
                provider: provider.clone(),
                holder: ProcessStamp::current()?,
                from_id: pf.outgoing.as_ref().map(|o| o.id.clone()),
                to_id: pf.target.id.clone(),
                from_fp: from_secret
                    .as_deref()
                    .and_then(|b| p.fingerprint(b))
                    .map(|f| f.as_str().to_owned()),
                from_identity: pf.live_identity.as_ref().map(|i| i.raw.clone()),
                to_fp: p
                    .fingerprint(target_secret)
                    .map(|f| f.as_str().to_owned())
                    .unwrap_or_default(),
                // §9.4 step 6: read under the target's account lock, so it cannot move before
                // the commit; a forward recovery records it (§9.6).
                to_epoch: Some(pf.target.login_epoch),
                started_at: self.now_ms(),
                prior: prior.clone(),
            },
        )?;
        Ok(prior)
    }

    /// The switch's result, taking the rows once nothing borrows them.
    fn outcome(
        &self,
        p: &dyn Provider,
        plan: &Plan,
        pf: Preflight,
        stored_in: SecretStore,
        warnings: Vec<String>,
    ) -> SwitchOutcome {
        let Preflight {
            managed_key_replaced,
            target,
            outgoing,
            ..
        } = pf;
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
        SwitchOutcome {
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
        }
    }

    /// Step 4: classify the outgoing credential and act on it. A capture's write changes the
    /// vault's fingerprint (it is never `Ours`), so it clears a quarantine (§7.4) and records
    /// the clear with the engine's source (review refactor §8.7).
    fn settle_outgoing(
        &self,
        cx: &mut Settle<'_>,
        ls: &LockedSwitch<'_, '_>,
        out: &AccountRow,
        live: &LiveAuth,
        hint: Option<&OracleHint>,
    ) -> Result<(), EngineError> {
        let p = cx.p;
        let store = ls.store;
        let Some(bytes) = Axis::of(p, &out.kind).live_secret(live) else {
            return Ok(());
        };
        let vault = match ls.memo.read(&out.id) {
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
            && match ls.memo.read_prev(&out.id) {
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
                let lock = ls
                    .account_locks
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
                self.unquarantine(out, CREDENTIALS_REPLACED)?;
                if backfill_uuid
                    && let Some(uuid) = resolved.and_then(|i| i.account_uuid.as_deref())
                {
                    store.backfill_account_uuid(&out.id, uuid)?;
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
                let identity = resolved.or(cx.live_identity).map(|i| &i.raw);
                let id = displace(
                    self,
                    &out.provider,
                    &bytes,
                    fp_live.as_ref(),
                    "displaced-live-login",
                    identity,
                )?;
                cx.warnings.push(if class == OutgoingClass::Foreign {
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

    /// Steps 7–9, keeping an undo for each write.
    fn apply<'a>(
        &self,
        p: &dyn Provider,
        step: ApplyStep<'_>,
        req: &SwitchRequest,
        tx: &mut Rollback<'a, '_>,
        before_fallback: BeforeFallback<'_>,
    ) -> Result<SecretStore, EngineError> {
        let ApplyStep {
            target,
            login: target_login,
            outgoing,
        } = step;
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
                at: self.now_s(),
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
                kind: EventKind::Switch,
                from_id: outgoing.map(|o| o.id.clone()),
                to_id: Some(target.id.clone()),
                trigger: Some(
                    req.auto
                        .as_ref()
                        .map_or("manual", |a| a.trigger.as_str())
                        .into(),
                ),
                source: self.source(),
                detail: req
                    .auto
                    .as_ref()
                    .and_then(|a| a.cause.as_ref())
                    .map(SwitchCause::detail),
            },
            record.as_ref(),
        )?;
        Ok(stored_in)
    }
}
