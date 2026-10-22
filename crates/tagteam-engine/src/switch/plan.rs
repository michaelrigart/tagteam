use tagteam_core::{AccountId, OracleVerdict};
use tagteam_provider::Provider;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::oracle::verdict;
use crate::store::{AccountRow, Store};

use super::select::{Ranked, Rotation};
use super::support::{Axis, OracleHint, answer_for};
use super::{
    SwitchOutcome, SwitchReason, SwitchRequest, SwitchTarget, already_active, noop, strategy_of,
    unmanaged_message,
};

/// Where planning gets its oracle answer.
#[allow(clippy::large_enum_variant)]
pub(super) enum Ask {
    /// Before the mutation lock: the one point where the oracle, and so the network, may be
    /// used (§9.4 "Before locking").
    Oracle,
    /// Under the mutation lock no network is used (§4.3, §9.4), so a re-plan keeps the answer
    /// an earlier attempt got; it still counts only for the bytes it was asked about.
    Reuse(Option<OracleHint>),
}

pub(super) struct Plan {
    pub(super) target: AccountRow,
    pub(super) strategy: &'static str,
    pub(super) self_switch: bool,
    pub(super) hint: Option<OracleHint>,
    /// The live account the plan was made against: a usage strategy's pick stands under the
    /// locks only while it still is. No network is used under the locks, so the ranking is not
    /// recomputed there; otherwise planning runs again from the store's readings.
    pub(super) anchor: Option<AccountId>,
    /// A rotation's accounts the walk read and passed over before its pick (§9.3). Empty for a
    /// direct target and a usage strategy.
    pub(super) walked: Vec<AccountId>,
    /// What a usage strategy's ranking tells the user (§9.3): accounts it skipped, candidates
    /// it could not rank, an unknown live headroom. Made afresh with every plan.
    pub(super) notes: Vec<String>,
    /// What freshening the target decided to tell the user (§7.2), carried into the outcome.
    pub(super) warnings: Vec<String>,
}

#[allow(clippy::large_enum_variant)]
pub(super) enum Planned {
    Done(SwitchOutcome),
    Go(Plan),
}

impl Engine {
    /// The target and the §9.2 special cases, decided from the current state.
    pub(super) fn plan(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
        ask: Ask,
    ) -> Result<Planned, EngineError> {
        let strategy = strategy_of(&req.target);
        let (live, live_row) = self.live_row(p, store, &req.provider)?;
        if let Some(refused) = self.auto_refusal(p, store, req, live_row.as_ref())? {
            return Ok(Planned::Done(refused));
        }
        let unmanaged_email = match (&live, &live_row) {
            (Some(i), None) => Some(i.display_email().to_owned()),
            _ => None,
        };
        let done = |reason: SwitchReason, message: String, warnings: Vec<String>| {
            let mut outcome = noop(
                strategy,
                reason,
                message,
                live_row.clone(),
                unmanaged_email.clone(),
            );
            outcome.warnings = warnings;
            Planned::Done(outcome)
        };
        if let (Some(email), false) = (&unmanaged_email, req.force) {
            return Ok(done(
                SwitchReason::UnmanagedAccount,
                unmanaged_message(email),
                Vec::new(),
            ));
        }
        let mut walked = Vec::new();
        let mut notes = Vec::new();
        let target = match &req.target {
            SwitchTarget::Account(id) => {
                // A switch never crosses providers (§9.3).
                let target = store
                    .account(id)?
                    .filter(|a| a.provider == req.provider)
                    .ok_or_else(|| EngineError::NoSuchAccount(id.to_string()))?;
                // §9.2: a session-owned target is refused, with or without --force, before its
                // vault is read. `rederive` checks again under the locks.
                self.refuse_session_owned(p, &target)?;
                target
            }
            SwitchTarget::Rotation => {
                match self.rotation(p, store, &req.provider, live_row.as_ref())? {
                    Rotation::To(a, passed) => {
                        walked = passed;
                        a
                    }
                    Rotation::Stay(reason, message) => {
                        return Ok(done(reason, message.into(), Vec::new()));
                    }
                }
            }
            // §9.3: `--model` replaces `autoswitch.models` for this switch, when given.
            SwitchTarget::Usage {
                strategy: by,
                models,
            } => {
                let models = models
                    .clone()
                    .unwrap_or_else(|| self.settings().models.clone());
                match self.usage_pick(p, store, &req.provider, live_row.as_ref(), *by, &models)? {
                    Ranked::To(a, said) => {
                        notes = said;
                        a
                    }
                    Ranked::Stay(reason, message, said) => {
                        return Ok(done(reason, message, said));
                    }
                }
            }
        };
        if !self.has_login(&target, None)? {
            return Err(EngineError::NoStoredCredential(format!(
                "{} cannot be activated: it has no stored credential; log in and run `tagteam add` again",
                target.label
            )));
        }
        // The direct branch displaces whatever is live, so it has nothing to ask about.
        let hint = match ask {
            _ if req.force => None,
            Ask::Oracle => match live_row.as_ref() {
                Some(out) => self.oracle_hint(p, out)?,
                None => None,
            },
            Ask::Reuse(hint) => hint,
        };
        let self_switch = live_row.as_ref().is_some_and(|r| r.id == target.id);
        if self_switch && !req.force {
            // A no-op unless the live credential diverged from the vault and the oracle
            // attributed it to this very account; then a full switch reconciles it (§9.2).
            let reconcile = Axis::of(p, &target.kind)
                .live_secret(&p.read_live_auth(&self.env))
                .is_some_and(|live| {
                    !self.matches_vault(p, &target, &live)
                        && verdict(answer_for(hint.as_ref(), &live), &target)
                            == OracleVerdict::ThisAccount
                });
            if !reconcile {
                return Ok(done(
                    SwitchReason::AlreadyActive,
                    already_active(&target),
                    Vec::new(),
                ));
            }
        }
        Ok(Planned::Go(Plan {
            anchor: live_row.as_ref().map(|r| r.id.clone()),
            target,
            strategy,
            self_switch,
            hint,
            walked,
            notes,
            warnings: vec![],
        }))
    }
}
