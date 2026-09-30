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

/// A successor a refresh has received and not yet persisted: the gate's (§7.3) and the
/// active-token refresh's (§7.5, Task 16). §7.3's rule is that it is never discarded: dropped
/// while still armed, by a panic or an early return, it keeps itself (`keep`). It holds a
/// secret, so it has no `Debug`.
pub(crate) struct Received<'e> {
    engine: &'e Engine,
    row: AccountRow,
    predecessor_fp: String,
    bytes: Vec<u8>,
    fp: Fingerprint,
    /// The owner the response named, when it is another account (§7.4). Such a successor is
    /// kept in `displaced/`, never in this account's vault or in `rescue/`.
    foreign: Option<Identity>,
    armed: bool,
}

impl<'e> Received<'e> {
    /// Arms the guard. Build it the moment the response is parsed, before any fallible step.
    /// `foreign` is the response's owner when `names_another_account` says it is not this
    /// account; it is fixed here, so no later path can store the successor as this account's.
    pub(crate) fn new(
        engine: &'e Engine,
        p: &dyn Provider,
        row: &AccountRow,
        predecessor_fp: &str,
        bytes: Vec<u8>,
        foreign: Option<Identity>,
    ) -> Self {
        let fp = p
            .fingerprint(&bytes)
            .unwrap_or_else(|| Fingerprint::of_secret(&bytes));
        Self {
            engine,
            row: row.clone(),
            predecessor_fp: predecessor_fp.to_owned(),
            bytes,
            fp,
            foreign,
            armed: true,
        }
    }

    /// Whether the response said the successor belongs to another account (§7.4).
    pub(crate) fn is_foreign(&self) -> bool {
        self.foreign.is_some()
    }

    /// One attempt to keep the successor outside the vault: `rescue/`, or `displaced/` when it
    /// belongs to another account (§7.3 step 6). It disarms whatever the result, so `Drop`
    /// never repeats it.
    pub(crate) fn keep(&mut self) -> Result<(), EngineError> {
        self.armed = false;
        match &self.foreign {
            Some(owner) => self
                .engine
                .keep_foreign(&self.row, &self.bytes, Some(&self.fp), owner),
            None => self
                .engine
                .write_rescue(
                    &self.row.id,
                    self.row.login_epoch,
                    &self.predecessor_fp,
                    &self.bytes,
                    &self.fp,
                )
                .map(drop),
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Received<'_> {
    /// The successor's bytes, for publishing it to the live store once persisted (§7.5
    /// step 5).
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for Received<'_> {
    /// Runs only while armed: a panic unwound past the successor before it was stored. It
    /// cannot return `Unpersisted`, so it keeps the successor if it can, and otherwise records
    /// the loss: an ERROR log line, which reaches stderr, and the account's quarantine (§7.3
    /// step 6, §7.4). It must never panic itself, since a second panic while unwinding aborts
    /// the process, so it makes only store writes and log lines, through calls that return
    /// errors instead of panicking.
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let engine = self.engine;
        let row = self.row.clone();
        let sent_fp = self.predecessor_fp.clone();
        let foreign = self.is_foreign();
        let kept = self.keep();
        if foreign {
            engine.quarantine_best_effort(&row, QuarantineReason::IdentityConflict, &sent_fp);
        }
        match kept {
            Ok(()) => tracing::error!(
                position = row.position,
                account = %row.id,
                "a refresh was interrupted before its token was stored; the token was kept outside the vault"
            ),
            Err(e) if foreign => log_lost(&row, &e),
            Err(e) => engine.record_loss(&row, &sent_fp, &e),
        }
    }
}

/// Where a received successor landed (§7.3 step 6, §7.5 step 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Persisted {
    /// The vault holds it.
    Vault,
    /// The vault could not take it, so `rescue/` holds it. The vault's generation is consumed.
    Rescued,
    /// Neither could. Not yet a loss: the caller decides, since active-token refresh may still
    /// publish it to the live store (§7.5 step 5), and records one with `record_loss`.
    Unpersisted,
}

/// What became of a successor an error interrupted before it was stored (§7.3 step 6).
pub(crate) enum Abandoned {
    /// Kept in `rescue/` (or `displaced/`): the caller returns the error.
    Kept(EngineError),
    /// Kept nowhere, and recorded: the caller reports `Unpersisted`, never the error.
    Lost,
}

/// What became of a successor that belongs to another account (§7.4).
pub(crate) enum Displacement {
    /// In `displaced/`, and the account quarantined `identity_conflict`.
    Kept,
    /// Kept nowhere: the caller reports `Unpersisted`.
    Lost,
}

/// A successor is lost (§7.3 step 6). Logged at ERROR, naming the account by position and ID
/// only (§4.4); the caller's refusal or notice carries it to the user.
pub(crate) fn log_lost(row: &AccountRow, cause: &dyn std::fmt::Display) {
    tracing::error!(
        position = row.position,
        account = %row.id,
        "a refreshed token was lost: {cause}"
    );
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
        let outcome = match result {
            RefreshResult::Refreshed { successor, owner } => {
                // §7.4: a successor the response says belongs to another account is marked
                // first, so no path, `Drop` included, ever stores it as this account's.
                let foreign = owner.filter(|o| names_another_account(o, &row));
                // From here on the successor is never discarded (§7.3): `received` keeps it if
                // this unwinds, and `abandon` if an error returns early.
                let mut received = Received::new(self, p, &row, &sent_fp, successor, foreign);
                let persisted = hooks::point(self, "gate-after-response")
                    .and_then(|()| self.persist_successor(p, &row, &lock, &sent_fp, &mut received));
                match persisted {
                    Ok(outcome) => outcome,
                    Err(e) if received.armed => {
                        match self.abandon(&row, &sent_fp, &mut received, e) {
                            Abandoned::Kept(e) => return Err(e),
                            Abandoned::Lost => GateOutcome::Unpersisted,
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
            other => {
                hooks::point(self, "gate-after-response")?;
                self.verdict(p, &row, &sent_fp, other)?
            }
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

    /// §7.3 step 6 and §7.5 step 5: the received successor goes to the vault, or to `rescue/`
    /// when the vault cannot take it. A failed `persist_generation` whose vault write landed is
    /// no loss: the metadata failure is logged and the result is `Vault`. `received` is
    /// disarmed whatever the result, so its `Drop` never writes a second copy. `Unpersisted` is
    /// not yet a loss: the gate records one at once (`lose`); active-token refresh only when the
    /// live store did not take the successor either (§7.5 step 5). A successor that belongs to
    /// another account never comes here: its caller keeps it with `Received::keep` (§7.4).
    pub(crate) fn persist_received(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        received: &mut Received<'_>,
    ) -> Persisted {
        debug_assert!(
            !received.is_foreign(),
            "a foreign successor is never stored"
        );
        let Err(e) = self.persist_generation(p, row, lock, &received.bytes) else {
            received.disarm();
            return Persisted::Vault;
        };
        // The vault write may have landed before recording it failed; then nothing is lost.
        if matches!(self.vault.read(&row.id), Read::Present(b) if b == received.bytes) {
            tracing::error!(
                position = row.position,
                account = %row.id,
                "a refreshed token was stored, but recording it failed: {e}"
            );
            received.disarm();
            return Persisted::Vault;
        }
        tracing::error!(
            position = row.position,
            account = %row.id,
            "the vault could not store a refreshed token: {e}"
        );
        match received.keep() {
            Ok(()) => Persisted::Rescued,
            Err(e) => {
                tracing::error!(
                    position = row.position,
                    account = %row.id,
                    "neither the vault nor rescue/ could store a refreshed token: {e}"
                );
                Persisted::Unpersisted
            }
        }
    }

    /// §7.3 step 6 for the gate: a successor that belongs to another account is displaced and
    /// the account quarantined; any other is persisted compare-and-swap style. Every path ends
    /// with the successor in the vault, in `rescue/` or `displaced/`, or reported as
    /// `Unpersisted`.
    fn persist_successor(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        sent_fp: &str,
        received: &mut Received<'_>,
    ) -> Result<GateOutcome, EngineError> {
        if received.is_foreign() {
            return Ok(match self.displace_received(row, sent_fp, received)? {
                Displacement::Kept => GateOutcome::Dead(QuarantineReason::IdentityConflict),
                Displacement::Lost => GateOutcome::Unpersisted,
            });
        }
        // Every vault writer holds the account lock, so this comparison cannot fail; it stays
        // as a defence.
        match self.vault.read(&row.id) {
            Read::Present(now) if fp_str(p, &now) == sent_fp => {}
            Read::Present(now) => {
                tracing::error!(
                    position = row.position,
                    account = %row.id,
                    "the vault moved while its refresh was in flight; the successor was kept in rescue/"
                );
                // The vault moved on to a generation this refresh did not consume, so losing
                // the successor here quarantines nothing.
                return Ok(match received.keep() {
                    Ok(()) => GateOutcome::AlreadyFresh(now),
                    Err(e) => {
                        log_lost(row, &e);
                        GateOutcome::Unpersisted
                    }
                });
            }
            Read::Absent | Read::Unreadable(_) => {
                return Ok(match received.keep() {
                    Ok(()) => GateOutcome::Transient {
                        kind: "vault-unreadable".into(),
                        rescued: true,
                    },
                    Err(e) => self.lose(row, sent_fp, &e),
                });
            }
        }
        hooks::point(self, "gate-before-vault-write")?;
        Ok(match self.persist_received(p, row, lock, received) {
            Persisted::Vault => GateOutcome::Refreshed(std::mem::take(&mut received.bytes)),
            Persisted::Rescued => GateOutcome::Transient {
                kind: "vault-write".into(),
                rescued: true,
            },
            Persisted::Unpersisted => self.lose(
                row,
                sent_fp,
                &"neither the vault nor rescue/ could store it",
            ),
        })
    }

    /// §7.3 step 6 and §7.4 for a successor that belongs to another account: it is displaced,
    /// never stored, and the account is quarantined `identity_conflict`, bound to `sent_fp`.
    /// A successor that could not even be displaced is reported first (`Lost`); the account is
    /// then still quarantined, so the loss is only logged. Shared by the gate and active-token
    /// refresh (Task 16).
    pub(crate) fn displace_received(
        &self,
        row: &AccountRow,
        sent_fp: &str,
        received: &mut Received<'_>,
    ) -> Result<Displacement, EngineError> {
        let kept = received.keep();
        let quarantined = self.quarantine(row, QuarantineReason::IdentityConflict, sent_fp);
        match (kept, quarantined) {
            (Err(e), _) => {
                log_lost(row, &e);
                Ok(Displacement::Lost)
            }
            (Ok(()), Err(e)) => Err(e),
            (Ok(()), Ok(())) => Ok(Displacement::Kept),
        }
    }

    /// Quarantines `row`, best effort: a failure is logged (position and ID only), never
    /// returned. Never panics, so `Received::drop` may call it while unwinding.
    pub(crate) fn quarantine_best_effort(
        &self,
        row: &AccountRow,
        reason: QuarantineReason,
        fp: &str,
    ) {
        if let Err(e) = self.quarantine(row, reason, fp) {
            tracing::error!(
                position = row.position,
                account = %row.id,
                reason = reason.as_str(),
                "could not quarantine the account: {e}"
            );
        }
    }

    /// §7.3 step 6 and §7.4 `successor_lost`: a received successor could be kept nowhere, and
    /// the generation that was sent, still the vault's, is consumed. Logs the loss and
    /// quarantines the account, bound to `sent_fp`, best effort: a store that cannot record it
    /// either leaves the loss reported only. Shared by the gate, `Received::drop` and
    /// active-token refresh (Task 16). Never panics.
    pub(crate) fn record_loss(
        &self,
        row: &AccountRow,
        sent_fp: &str,
        cause: &dyn std::fmt::Display,
    ) {
        log_lost(row, cause);
        self.quarantine_best_effort(row, QuarantineReason::SuccessorLost, sent_fp);
    }

    /// The gate's `Unpersisted`, recorded (`record_loss`).
    fn lose(&self, row: &AccountRow, sent_fp: &str, cause: &dyn std::fmt::Display) -> GateOutcome {
        self.record_loss(row, sent_fp, cause);
        GateOutcome::Unpersisted
    }

    /// §7.3 step 6: an error after the response was received, before the successor was
    /// stored. The successor is kept first (`Received::keep`) and the caller returns the error.
    /// If keeping it fails too, the loss is recorded (`record_loss`; only logged for a foreign
    /// successor, whose account is quarantined `identity_conflict` instead) and the caller
    /// reports `Unpersisted`, never the error. Shared by the gate and active-token refresh
    /// (Task 16).
    pub(crate) fn abandon(
        &self,
        row: &AccountRow,
        sent_fp: &str,
        received: &mut Received<'_>,
        cause: EngineError,
    ) -> Abandoned {
        let foreign = received.is_foreign();
        let kept = received.keep();
        if foreign {
            self.quarantine_best_effort(row, QuarantineReason::IdentityConflict, sent_fp);
        }
        match kept {
            Ok(()) => Abandoned::Kept(cause),
            Err(e) => {
                tracing::error!(
                    position = row.position,
                    account = %row.id,
                    "the refresh failed after its token was received: {cause}"
                );
                if foreign {
                    log_lost(row, &e);
                } else {
                    self.record_loss(row, sent_fp, &e);
                }
                Abandoned::Lost
            }
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
