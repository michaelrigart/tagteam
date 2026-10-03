//! Active-token refresh (§7.5): the one path that ever sends the live login's refresh token.

use std::path::PathBuf;
use std::time::Duration;

use tagteam_core::{AccountId, Fingerprint, OracleVerdict, ProviderId};
use tagteam_provider::{
    Cancel, CredLocks, Credential, LiveChange, Provenance, Provider, ProviderError, Read,
    RefreshResult, StoredLogin,
};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::oracle::verdict;
use crate::quarantine::QuarantineReason;
use crate::refresh::{
    Abandoned, Displacement, Persisted, Received, expired, names_another_account,
};
use crate::rescue::{RescueEntry, RescueFile};
use crate::store::AccountRow;
use crate::switch::{Held, OracleHint, answer_for, refuse_unreadable};

/// §7.5 step 5: how long the token request may take while CC's credential locks are held
/// (§4.3's second bounded exception).
pub const ACTIVE_REFRESH_TIMEOUT: Duration = Duration::from_secs(6);

/// Why the caller asks (§7.5): the only two reasons tagteam refreshes the live token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveTrigger {
    /// The live access token has expired.
    Expired,
    /// The server answered 401 to this access token although it is still valid locally
    /// (a sibling machine revoked it; §8.1 `rejected_fp`).
    Rejected { access_fp: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveOutcome {
    /// Reconciliation left a live token that is neither expired nor the rejected one.
    NotNeeded {
        reconciled: bool,
    },
    /// The successor is in tagteam's storage (the vault, or `rescue/`) and in the live store.
    Refreshed,
    /// CC's locks were compromised when the response arrived: persisted to the vault or
    /// `rescue/`, but not published; the next pass reconciles (§7.5 step 5).
    PersistedNotPublished,
    /// Neither the vault nor `rescue/` could take the successor, but the live store holds it,
    /// so nothing is lost: the next pass adopts it into the vault (§7.5 step 3). Nothing is
    /// quarantined.
    PublishedOnly,
    Dead(QuarantineReason),
    Systemic(String),
    Transient {
        kind: String,
    },
    /// The successor is held nowhere: logged at ERROR, and the account quarantined
    /// `successor_lost` (§7.5 step 5, §7.4).
    Unpersisted,
    /// §7.5 step 2: the account's activation epoch is stale (§12.5). An explicit command
    /// replaced its login while Claude Code kept the old one, and adopting or refreshing that
    /// lineage would undo the replacement. Nothing was read further, sent or written; Claude
    /// Code goes on refreshing its own copy until `tagteam switch <N> --force`.
    Replaced,
}

/// A successor neither the vault nor `rescue/` could take, while its live write is under way.
/// If that write unwinds, dropping this records the loss (§7.3 step 6: `successor_lost`, bound
/// to the generation sent). Disarmed once the outcome is settled.
struct PendingLoss<'a> {
    engine: &'a Engine,
    row: &'a AccountRow,
    sent_fp: &'a str,
    armed: bool,
}

impl Drop for PendingLoss<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.engine.record_loss(
                self.row,
                self.sent_fp,
                &"publishing it to the live store unwound",
            );
        }
    }
}

/// What reconciliation (§7.5 step 3) settled: the generation the live store holds, or will
/// hold once `publish` writes it, and the rescue files to retire once that write reads back.
struct Reconciled {
    current: Vec<u8>,
    publish: bool,
    changed: bool,
    retire: Vec<PathBuf>,
}

/// §14.2's refresh outcome for §7.5, one line per call, naming the provider and, once it is
/// known, the live account. INFO once a request was sent or state changed, and for an error,
/// which is named by its `kind()` alone; DEBUG for a token that needed nothing, and for
/// `Replaced`, since §7.5 step 2 sends and writes nothing. A systemic refusal's own words are
/// never logged.
fn log_active(
    provider: &ProviderId,
    account: Option<&AccountId>,
    result: &Result<ActiveOutcome, EngineError>,
) {
    const OUTCOME: &str = "active-token refresh outcome";
    let account = account.map(tracing::field::display);
    match result {
        Ok(ActiveOutcome::Refreshed) => {
            tracing::info!(provider = %provider, account, outcome = "refreshed", "{OUTCOME}");
        }
        Ok(ActiveOutcome::PersistedNotPublished) => tracing::info!(
            provider = %provider,
            account,
            outcome = "persisted-not-published",
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::PublishedOnly) => tracing::info!(
            provider = %provider,
            account,
            outcome = "published-only",
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::NotNeeded { reconciled: true }) => tracing::info!(
            provider = %provider,
            account,
            outcome = "not-needed",
            reconciled = true,
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::Dead(reason)) => tracing::info!(
            provider = %provider,
            account,
            outcome = "dead",
            reason = reason.as_str(),
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::Systemic(_)) => {
            tracing::info!(provider = %provider, account, outcome = "systemic", "{OUTCOME}");
        }
        Ok(ActiveOutcome::Transient { kind }) => tracing::info!(
            provider = %provider,
            account,
            outcome = "transient",
            kind = kind.as_str(),
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::Unpersisted) => {
            tracing::info!(provider = %provider, account, outcome = "unpersisted", "{OUTCOME}");
        }
        Err(e) => tracing::info!(
            provider = %provider,
            account,
            outcome = "error",
            kind = e.kind(),
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::Replaced) => tracing::debug!(
            provider = %provider,
            account,
            outcome = "replaced",
            "{OUTCOME}"
        ),
        Ok(ActiveOutcome::NotNeeded { reconciled: false }) => tracing::debug!(
            provider = %provider,
            account,
            outcome = "not-needed",
            "{OUTCOME}"
        ),
    }
}

impl Engine {
    /// §7.5. The mutation lock (recovering first), the live account's lock, then CC's
    /// credential locks only; the config lock is taken after the request, around the live
    /// write alone. The oracle is asked before any lock (§7.6). Whichever way it ends, its
    /// outcome is logged once (`log_active`), after every lock is released.
    ///
    /// Inside a run shell it runs as outside one (§12.8, B.57): `env` is the outer home
    /// (Decision 6), so the live login it reads, refreshes and protects is the default home's,
    /// never the session's profile. Only a marker that cannot be read refuses.
    pub fn refresh_active(
        &self,
        provider: &ProviderId,
        trigger: ActiveTrigger,
    ) -> Result<ActiveOutcome, EngineError> {
        let mut account = None;
        let result = self.run_active_refresh(provider, trigger, &mut account);
        log_active(provider, account.as_ref(), &result);
        result
    }

    /// `refresh_active`'s steps. `account` is set as soon as the live login's account is
    /// known, so the caller's line names it however the refresh ends.
    fn run_active_refresh(
        &self,
        provider: &ProviderId,
        trigger: ActiveTrigger,
        account: &mut Option<AccountId>,
    ) -> Result<ActiveOutcome, EngineError> {
        self.refuse_unreadable_run_shell()?;
        let provider_arc = self.provider(provider)?;
        let p = provider_arc.as_ref();
        let (row, live) = self.active_login(p, provider)?;
        *account = Some(row.id.clone());
        let hint = self.corroborate(p, &row, &live)?;

        // Step 1: the mutation lock and the account lock, held throughout.
        let guard = self.guard_or_refuse(provider)?;
        let lock = self.lock_account(&row.id)?;
        // At most two passes. Publishing a recovered generation (step 3's self-heal) releases
        // CC's credential locks along with the config lock, so a refresh that is still needed
        // afterwards runs in a second pass, from a fresh re-read under freshly taken locks. That
        // pass finds the live store in step with the vault, so it never publishes again.
        for _pass in 0..2 {
            let cred = p.lock_credentials(&self.env, &guard, p.live_lock_budget())?;

            // Step 2: the same account, read fresh, now that CC cannot rotate it.
            let (row, live) = self.active_login(p, provider)?;
            if &row.id != lock.id() {
                return Err(EngineError::LiveMoved);
            }
            // §12.5: a replacement superseded the lineage the live store holds. `row` was read
            // under the account lock, so its epoch cannot move before this returns.
            if self.store()?.live_store_stale(&row)? {
                return Ok(ActiveOutcome::Replaced);
            }
            // §7.4: a quarantined generation is never sent again.
            if let Some(reason) = self.active_quarantine(p, &row, &live)? {
                return Ok(ActiveOutcome::Dead(reason));
            }

            // Step 3.
            let rec = self.reconcile_active(p, &row, &lock, &live, hint.as_ref())?;

            // Step 4: a request only if the newest generation still needs one.
            let now = self.now_ms();
            let rejected = match &trigger {
                ActiveTrigger::Rejected { access_fp } => p
                    .access_fingerprint(&rec.current)
                    .is_some_and(|f| f.as_str() == access_fp),
                ActiveTrigger::Expired => false,
            };
            let needed = rejected || expired(p, &rec.current, now);
            if rec.publish {
                // The self-heal comes first, needed or not: advancing the lineage again before
                // the live store caught up would leave it two generations behind, where no
                // later pass could tell its generation from a CC rotation.
                // Nothing was sent this pass, so a signal may end this wait (§14.1).
                if !self.publish(p, &row, cred, &rec.current, &rec.retire, &self.env.cancel)? {
                    return Ok(ActiveOutcome::PersistedNotPublished);
                }
                if needed {
                    continue;
                }
                return Ok(ActiveOutcome::NotNeeded { reconciled: true });
            }
            if !needed {
                return Ok(ActiveOutcome::NotNeeded {
                    reconciled: rec.changed,
                });
            }

            // Step 5.
            return self.request_active(p, &row, &lock, cred, &rec, now);
        }
        Err(EngineError::LiveMoved)
    }

    /// §7.4: the active account's quarantine holds while the live credential or the vault
    /// still carries the generation it is bound to (a quarantine with no bound generation
    /// always holds). Such a token is never sent again.
    fn active_quarantine(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        live: &[u8],
    ) -> Result<Option<QuarantineReason>, EngineError> {
        let Some(reason) = &row.quarantine_reason else {
            return Ok(None);
        };
        let reason = QuarantineReason::parse(reason).unwrap_or(QuarantineReason::InvalidGrant);
        let Some(bound) = &row.quarantine_fp else {
            return Ok(Some(reason));
        };
        let carries = |b: &[u8]| p.fingerprint(b).is_some_and(|f| f.as_str() == bound);
        let vault_carries = match self.vault.read(&row.id) {
            Read::Present(v) => carries(&v),
            Read::Absent => false,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        Ok((carries(live) || vault_carries).then_some(reason))
    }

    /// §7.5 step 5: the request under CC's credential locks only (6 s), then persistence, then
    /// the live write under the config lock.
    fn request_active(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        cred: CredLocks<'_>,
        rec: &Reconciled,
        now: i64,
    ) -> Result<ActiveOutcome, EngineError> {
        cred.check_owned()?;
        hooks::point(self, "active-before-request")?;
        let sent = p.fingerprint(&rec.current);
        let sent_fp = sent.as_ref().map(Fingerprint::as_str).unwrap_or_default();
        // The type cannot prove `rec.current` fresh here: it arrives as bytes through
        // `Reconciled`, not as a `FreshCredential`. Freshness rests on the runtime check in
        // `active_login`, which refuses a degraded or empty live read before anything is sent
        // (§4.3); the other sources are vault reads, which are authoritative.
        // `a_degraded_live_read_is_refused_before_any_request` pins that check, so a change to
        // it that let a degraded token through would fail that test.
        let fresh = Credential::fresh(rec.current.clone())
            .into_fresh()
            .expect("a credential built fresh is fresh");
        match p.refresh(&*self.http, &fresh, now, ACTIVE_REFRESH_TIMEOUT) {
            RefreshResult::Refreshed { successor, owner } => {
                // §7.4: a successor the response says belongs to another account is marked
                // first (Task 11's rule): it is displaced, never stored or published.
                let foreign = owner.filter(|o| names_another_account(o, row));
                // From here on the successor is never discarded (§7.3): `received` keeps it if
                // anything below unwinds before it is stored.
                let mut received = Received::new(self, p, row, sent_fp, successor, foreign);
                if let Err(e) = hooks::point(self, "active-after-response") {
                    // As the gate does (Task 11): keep it and return the error, or record the
                    // loss and report it, never the error.
                    return match self.abandon(row, sent_fp, &mut received, e) {
                        Abandoned::Kept(e) => Err(e),
                        Abandoned::Lost => Ok(ActiveOutcome::Unpersisted),
                    };
                }
                if received.is_foreign() {
                    // Task 11's shared path: displaced and quarantined `identity_conflict`; a
                    // successor that could not even be displaced is reported first (§7.3 step 6).
                    return Ok(match self.displace_received(row, sent_fp, &mut received)? {
                        Displacement::Kept => {
                            ActiveOutcome::Dead(QuarantineReason::IdentityConflict)
                        }
                        Displacement::Lost => ActiveOutcome::Unpersisted,
                    });
                }
                let owned = cred.check_owned().is_ok();
                // Task 11's shared step: the vault, else `rescue/`, else reported as lost.
                let persisted = self.persist_received(p, row, lock, &mut received);
                // `persist_received` disarmed `received`. When neither store took the
                // successor, the live write below is its last home, so the loss stays armed
                // until that write lands: a panic in between still records it (§7.3 step 6).
                let mut pending = PendingLoss {
                    engine: self,
                    row,
                    sent_fp,
                    armed: persisted == Persisted::Unpersisted,
                };
                // CC must hold the newest generation whatever became of tagteam's copy. The
                // request consumed the one CC holds, so this wait is inside §7.5's critical
                // span: it waits under a token nothing sets, and a signal waits for the next
                // cancellation point (§14.1). A timeout still leaves it to step 3's self-heal.
                let published = if owned {
                    let uncancelled = Cancel::new();
                    hooks::point(self, "active-before-publish").and_then(|()| {
                        self.publish(p, row, cred, received.bytes(), &rec.retire, &uncancelled)
                    })
                } else {
                    Ok(false)
                };
                // Settled below either way: published, or recorded by `record_loss`.
                pending.armed = false;
                // A loss takes precedence over everything else (§7.3 step 6). But a successor
                // the live store holds is not lost: CC has it, and the next pass adopts it into
                // the vault (§7.5 step 3). Only one held nowhere is recorded, as the gate
                // records it: `successor_lost`, bound to the generation sent (§7.5 step 5).
                Ok(match (persisted, published) {
                    (Persisted::Unpersisted, Ok(true)) => ActiveOutcome::PublishedOnly,
                    (Persisted::Unpersisted, published) => {
                        let cause = match &published {
                            Err(e) => format!("the live store was not written either: {e}"),
                            Ok(_) => "the live store was not written either".to_owned(),
                        };
                        self.record_loss(row, sent_fp, &cause);
                        ActiveOutcome::Unpersisted
                    }
                    (_, Err(e)) => return Err(e),
                    (_, Ok(true)) => ActiveOutcome::Refreshed,
                    (_, Ok(false)) => ActiveOutcome::PersistedNotPublished,
                })
            }
            RefreshResult::Dead(reason) => {
                // §7.3 step 7: Dead only while the source that was sent still holds the
                // generation sent; otherwise the lineage moved and this is a failed refresh.
                let holds = |bytes: Option<Vec<u8>>| {
                    sent.is_some() && bytes.as_deref().and_then(|b| p.fingerprint(b)) == sent
                };
                let live_now = p
                    .read_live_auth(&self.env)
                    .credential
                    .present()
                    .filter(|c| c.provenance() == Provenance::Fresh)
                    .map(|c| c.bytes().to_vec());
                if !holds(live_now) && !holds(self.vault.read(&row.id).present()) {
                    return Ok(ActiveOutcome::Transient {
                        kind: "refresh-failed".into(),
                    });
                }
                let reason: QuarantineReason = reason.into();
                self.quarantine(row, reason, sent_fp)?;
                Ok(ActiveOutcome::Dead(reason))
            }
            RefreshResult::Systemic(detail) => Ok(ActiveOutcome::Systemic(detail)),
            RefreshResult::Transient(kind) => Ok(ActiveOutcome::Transient { kind: kind.token() }),
        }
    }

    /// The live login's account and credential, read fresh (§7.5 step 2). Only a managed
    /// login of a kind that refreshes, from a read that is neither degraded nor empty (§4.3).
    fn active_login(
        &self,
        p: &dyn Provider,
        provider: &ProviderId,
    ) -> Result<(AccountRow, Vec<u8>), EngineError> {
        let identity = self
            .read_live_identity(p)?
            .ok_or(EngineError::NoLiveLogin)?;
        let row = match self.existing_store()? {
            Some(store) => {
                store.find_by_identity_key(provider, p.identity_key(&identity).as_str())?
            }
            None => None,
        }
        .ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "the live login is not managed by tagteam; only {} refreshes it",
                p.display_name()
            ))
        })?;
        if !p.kind_traits(&row.kind).refreshable {
            return Err(EngineError::InvalidInput(format!(
                "position {} holds a credential that does not refresh",
                row.position
            )));
        }
        let bytes = match p.read_live_auth(&self.env).credential {
            Read::Present(c) if c.provenance() == Provenance::Degraded => {
                return Err(EngineError::DegradedRead);
            }
            Read::Present(c) if !c.is_empty() => c.bytes().to_vec(),
            Read::Present(_) | Read::Absent => return Err(EngineError::NoLiveLogin),
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        Ok((row, bytes))
    }

    /// The oracle's word on the live credential, asked before any lock and only while its
    /// access token can still be shown (§7.6). An answer naming someone else refuses at once.
    fn corroborate(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        live: &[u8],
    ) -> Result<Option<OracleHint>, EngineError> {
        if expired(p, live, self.now_ms()) {
            return Ok(None);
        }
        let resolved = self.oracle.resolve(p, &Credential::fresh(live.to_vec()));
        if verdict(resolved.as_ref(), row) == OracleVerdict::OtherIdentity {
            return Err(EngineError::ForeignLiveCredential {
                position: row.position,
            });
        }
        Ok(Some(OracleHint {
            bytes: live.to_vec(),
            resolved,
        }))
    }

    /// §7.5 step 3's table, before any request. Generation order comes from the live store and
    /// fingerprints, never from access-token expiry (B.48).
    fn reconcile_active(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        live: &[u8],
        hint: Option<&OracleHint>,
    ) -> Result<Reconciled, EngineError> {
        let fp = |b: &[u8]| p.fingerprint(b);
        let relogin = || EngineError::NeedsRelogin {
            position: row.position,
            label: row.label.clone(),
        };
        let live_fp = fp(live).ok_or_else(relogin)?;
        let vault = match self.vault.read(&row.id) {
            Read::Present(v) => Some(v),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        // Tri-state (B.1): an unreadable `.prev` may be the generation that tells an
        // unpublished earlier pass apart from a CC rotation, so it refuses rather than reads as
        // absent, which would adopt a consumed token as the newest.
        let prev = match self.vault.read_prev(&row.id) {
            Read::Present(v) => Some(v),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let mut rescues: Vec<RescueEntry> = Vec::new();
        for file in self.rescues_for(&row.id) {
            match file {
                RescueFile::Entry(entry) => rescues.push(entry),
                RescueFile::Unreadable { path, detail } => {
                    return Err(EngineError::RescuePending {
                        position: row.position,
                        label: row.label.clone(),
                        detail: format!("{}: {detail}", path.display()),
                    });
                }
            }
        }
        let is_live = |b: &[u8]| fp(b).as_ref() == Some(&live_fp);

        // In step with the vault. A rescue that succeeds this generation is published: the
        // vault now, the live store once the config lock is taken.
        if vault.as_deref().is_some_and(is_live) {
            if let Some(r) = rescues
                .iter()
                .find(|r| r.predecessor_fp == live_fp.as_str())
            {
                self.persist_generation(p, row, lock, &r.credential)?;
                return Ok(Reconciled {
                    current: r.credential.clone(),
                    publish: true,
                    changed: true,
                    retire: vec![r.path.clone()],
                });
            }
            // A rescue of exactly this generation has landed everywhere.
            let landed: Vec<&RescueEntry> =
                rescues.iter().filter(|r| is_live(&r.credential)).collect();
            for r in &landed {
                self.delete_rescue(&r.path)?;
            }
            return Ok(Reconciled {
                current: live.to_vec(),
                publish: false,
                changed: !landed.is_empty(),
                retire: vec![],
            });
        }

        // The vault's `.prev`: an earlier pass reached the vault but not the live store.
        if let (Some(v), Some(pr)) = (&vault, &prev) {
            if is_live(pr) {
                let vault_fp = fp(v);
                let retire = rescues
                    .iter()
                    .filter(|r| vault_fp.is_some() && fp(&r.credential) == vault_fp)
                    .map(|r| r.path.clone())
                    .collect();
                return Ok(Reconciled {
                    current: v.clone(),
                    publish: true,
                    changed: true,
                    retire,
                });
            }
        }

        // A rescue's generation: published, but its vault write failed.
        if let Some(r) = rescues.iter().find(|r| is_live(&r.credential)) {
            self.persist_generation(p, row, lock, &r.credential)?;
            self.delete_rescue(&r.path)?;
            return Ok(Reconciled {
                current: live.to_vec(),
                publish: false,
                changed: true,
                retire: vec![],
            });
        }

        // Any other full token pair: CC rotated it, so it is the newest generation, adopted
        // whatever its access token's expiry. An access-token-only blob never replaces the
        // vault's refresh token (§6.2).
        if !p.has_refresh_token(live) {
            return Err(relogin());
        }
        if verdict(answer_for(hint, live), row) == OracleVerdict::OtherIdentity {
            return Err(EngineError::ForeignLiveCredential {
                position: row.position,
            });
        }
        self.persist_generation(p, row, lock, live)?;
        for r in &rescues {
            self.delete_rescue(&r.path)?;
        }
        Ok(Reconciled {
            current: live.to_vec(),
            publish: false,
            changed: true,
            retire: vec![],
        })
    }

    /// Writes `secret` to the live store under the config lock, taken now with its own budget
    /// (§9.1) and waited for under `cancel` (§14.1), then retires `retire` once the write reads
    /// back. `false` when the live store was not written; the caller reports
    /// `PersistedNotPublished`, and the next pass reconciles it (§7.5 step 3). What the write
    /// destroys is saved first unless a vault generation already holds it (§9.4 step 7's rule).
    fn publish(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        cred: CredLocks<'_>,
        secret: &[u8],
        retire: &[PathBuf],
        cancel: &Cancel,
    ) -> Result<bool, EngineError> {
        let not_published = |why: &dyn std::fmt::Display| {
            tracing::warn!(
                position = row.position,
                account = %row.id,
                "a refreshed credential was not published to the live store: {why}"
            );
            Ok(false)
        };
        let mut env = self.env.clone();
        env.cancel = cancel.clone();
        let locks = match p.lock_config(&env, cred, p.live_lock_budget()) {
            Ok(locks) => locks,
            Err(e) => return not_published(&e),
        };
        let doomed = p.doomed(&self.env, &locks, LiveChange::Write(&row.kind));
        if let Err(e) = refuse_unreadable(&doomed) {
            return not_published(&e);
        }
        let mut held = Held::default();
        held.hold(p, secret);
        self.hold_vault(p, &mut held, &row.id);
        let mut warnings = Vec::new();
        for entry in &doomed {
            if let Read::Present(bytes) = &entry.bytes {
                if let Err(e) = self.save_unheld(
                    p,
                    &row.provider,
                    bytes,
                    &mut held,
                    false,
                    None,
                    None,
                    &mut warnings,
                ) {
                    return not_published(&e);
                }
            }
        }
        let login = StoredLogin {
            kind: row.kind.clone(),
            secret: secret.to_vec(),
            identity: p.parse_identity(&row.identity_json)?,
        };
        let written = {
            let mut before_fallback = |bytes: &[u8]| {
                self.save_unheld(
                    p,
                    &row.provider,
                    bytes,
                    &mut held,
                    false,
                    None,
                    None,
                    &mut warnings,
                )
                .map_err(|e| {
                    ProviderError::Invalid(format!(
                        "could not save a credential the Keychain fallback would delete: {e}"
                    ))
                })
            };
            // The undo is dropped, never run: it would write the consumed generation back. The
            // storage-write wait honours `cancel`, as the config-lock wait did (§9.1, §14.1).
            p.write_credential(&env, &locks, &login, &mut before_fallback)
                .map(|_| ())
        };
        for w in &warnings {
            tracing::warn!(
                position = row.position,
                account = %row.id,
                "publishing a refreshed credential: {w}"
            );
        }
        if let Err(e) = written {
            return not_published(&e);
        }
        let landed = match p.read_live_auth(&self.env).credential {
            Read::Present(c) if c.provenance() == Provenance::Fresh => {
                p.fingerprint(c.bytes()).is_some()
                    && p.fingerprint(c.bytes()) == p.fingerprint(secret)
            }
            _ => false,
        };
        if landed {
            for path in retire {
                self.delete_rescue(path)?;
            }
        }
        Ok(landed)
    }
}
