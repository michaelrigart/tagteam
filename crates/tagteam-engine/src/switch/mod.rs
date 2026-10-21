use tagteam_core::autoswitch::{Departure, Trigger};
use tagteam_core::poll::replan_for_role;
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::{Identity, MutationGuard, Provider, Read, SecretStore};

use crate::collect::jitter;
use crate::engine::{Engine, GuardRequest};
use crate::error::EngineError;
use crate::hooks;
use crate::store::{AccountRow, Store};

use plan::{Ask, Planned};
use transact::{LockedSwitch, Rederived};

mod freshen;
mod plan;
mod rollback;
mod select;
mod support;
mod transact;

pub(crate) use freshen::{AutoFreshened, FreshenFailure};
pub(crate) use support::{
    Axis, OracleHint, Settle, answer_for, cannot_refresh, needs_relogin, refuse_unreadable,
    refuse_unsafe_live_reads, works_until_expiry,
};

/// §9.3's strategies that rank by usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageStrategy {
    Best,
    NextAvailable,
}

impl UsageStrategy {
    /// The strategy as `switch --json` names it (§13.2).
    pub fn as_str(self) -> &'static str {
        match self {
            UsageStrategy::Best => "best",
            UsageStrategy::NextAvailable => "next-available",
        }
    }
}

#[derive(Debug, Clone)]
pub enum SwitchTarget {
    Rotation,
    Account(AccountId),
    /// §9.3: `models` overrides `autoswitch.models` for this switch (`--model`).
    Usage {
        strategy: UsageStrategy,
        models: Option<Vec<String>>,
    },
}

impl SwitchTarget {
    /// Rotation or a usage strategy: the engine chooses the account, so a pick that turns out
    /// dead or quarantined is replaced by planning again, where a named target is refused or
    /// warned about (§7.2, §9.3, §9.4 step 1).
    fn chosen(&self) -> bool {
        !matches!(self, SwitchTarget::Account(_))
    }
}

#[derive(Debug, Clone)]
pub struct SwitchRequest {
    pub provider: ProviderId,
    pub target: SwitchTarget,
    pub force: bool,
    pub source: &'static str,
    /// An automatic switch's preconditions (§11.2 step 11), checked under the locks before the
    /// first write; `None` for every other switch. Its target is a `SwitchTarget::Account`.
    pub auto: Option<AutoPerform>,
}

/// What an automatic switch rests on (§11.2 step 11): the live account the tick decided on,
/// its trigger, the cooldown to judge it by, and the departure snapshot its commit records for
/// the account it leaves (§11.3).
#[derive(Debug, Clone)]
pub struct AutoPerform {
    pub expected_from: AccountId,
    pub trigger: Trigger,
    pub cooldown_s: i64,
    pub departure: Departure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchReason {
    Switched,
    AlreadyActive,
    Activated,
    UnmanagedAccount,
    OnlyOneAccount,
    NoValidTarget,
    UsageUnavailable,
    AlreadyBest,
    CandidatesExhausted,
    /// An automatic switch's live account is no longer the one its tick decided on.
    LiveChanged,
    /// An automatic `proactive` or `consume-first` switch met the cooldown under the lock.
    Cooldown,
    /// An automatic switch's target is no longer a candidate (§11.2 step 7).
    NotCandidate,
}

impl SwitchReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SwitchReason::Switched => "switched",
            SwitchReason::AlreadyActive => "already-active",
            SwitchReason::Activated => "activated",
            SwitchReason::UnmanagedAccount => "unmanaged-account",
            SwitchReason::OnlyOneAccount => "only-one-account",
            SwitchReason::NoValidTarget => "no-valid-target",
            SwitchReason::UsageUnavailable => "usage-unavailable",
            SwitchReason::AlreadyBest => "already-best",
            SwitchReason::CandidatesExhausted => "candidates-exhausted",
            SwitchReason::LiveChanged => "live-changed",
            SwitchReason::Cooldown => "cooldown",
            SwitchReason::NotCandidate => "not-candidate",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SwitchOutcome {
    pub switched: bool,
    pub from: Option<AccountRow>,
    pub to: Option<AccountRow>,
    pub strategy: &'static str,
    pub reason: SwitchReason,
    pub message: String,
    pub warnings: Vec<String>,
    /// Where this switch's credential write put the secret; `None` when it wrote none.
    pub stored_in: Option<SecretStore>,
    pub unmanaged_email: Option<String>,
    /// A live managed API key was in place and this switch replaced or cleared it, whether or
    /// not a stored account held it. CC keeps that key for the life of its process (Appendix
    /// A.7), so the human output says to restart it.
    pub managed_key_replaced: bool,
    /// What the provider says activates this switch (its `ProviderWords`), chosen from the kinds
    /// on either side and where the secret went; `None` for a no-op.
    pub activation_hint: Option<&'static str>,
}

/// §9.4 step 1: after this many lock acquisitions that each found the plan outdated, abort.
const ATTEMPTS: usize = 3;

/// §9.2: fewer than two candidates, for a rotation or a usage strategy.
const ONLY_ONE: &str = "there is only one switchable account";
/// §9.3: no live login to anchor on, and nothing in the store to activate.
const NO_VALID: &str = "no account can be activated";
/// §9.3 `best`: no candidate's reading can drive a decision (§8.4).
const USAGE_UNAVAILABLE: &str = "no candidate has a usage reading recent enough to rank by";
const ONE_UNRANKED: &str =
    "1 candidate has no usage reading recent enough to rank by; it was not considered";
const LIVE_UNKNOWN: &str =
    "switching to the best known candidate; the live account's usage is unknown";
const NO_LIVE: &str =
    "switching to the best known candidate; there is no managed live login to compare with";
/// §11.2 step 11: the live account is no longer the one the tick decided on.
const LIVE_CHANGED: &str = "the live login changed after this switch was decided; a switch made meanwhile is never overridden";

fn unmanaged_message(email: &str) -> String {
    format!("the live login ({email}) is not managed by tagteam; add it first, or use --force")
}

fn login_email(i: &Identity) -> String {
    i.email.clone().unwrap_or_else(|| i.label.clone())
}

fn strategy_of(target: &SwitchTarget) -> &'static str {
    match target {
        SwitchTarget::Rotation => "rotation",
        SwitchTarget::Account(_) => "direct",
        SwitchTarget::Usage { strategy, .. } => strategy.as_str(),
    }
}

fn noop(
    strategy: &'static str,
    reason: SwitchReason,
    message: String,
    from: Option<AccountRow>,
    unmanaged_email: Option<String>,
) -> SwitchOutcome {
    SwitchOutcome {
        switched: false,
        from,
        to: None,
        strategy,
        reason,
        message,
        warnings: vec![],
        stored_in: None,
        unmanaged_email,
        managed_key_replaced: false,
        activation_hint: None,
    }
}

/// How a switch reaches the provider's running sessions: a change on the managed-key axis, on
/// either side or to a live key no account held, reaches only a new process, whichever store the
/// credential went to; otherwise the store decides.
fn activation_hint(
    p: &dyn Provider,
    from_kind: Option<&str>,
    to_kind: &str,
    stored_in: &SecretStore,
    managed_key_replaced: bool,
) -> &'static str {
    let words = p.words();
    let managed = [from_kind, Some(to_kind)]
        .into_iter()
        .flatten()
        .any(|kind| p.kind_traits(kind).managed_key_axis)
        || managed_key_replaced;
    match stored_in {
        _ if managed => words.after_switch_managed_key,
        SecretStore::File(_) | SecretStore::Fallback(_) => words.after_switch_file,
        SecretStore::Keychain => words.after_switch_keychain,
    }
}

fn already_active(target: &AccountRow) -> String {
    format!("{} is already active", target.label)
}

impl Engine {
    /// §5: with no store there is nothing to activate, and nothing is created; an unmanaged
    /// live login is still reported as one (§9.2).
    fn without_store(
        &self,
        p: &dyn Provider,
        req: &SwitchRequest,
    ) -> Result<SwitchOutcome, EngineError> {
        let unmanaged = self.read_live_identity(p)?.as_ref().map(login_email);
        let (reason, message) = match (&unmanaged, req.force) {
            (Some(email), false) => (SwitchReason::UnmanagedAccount, unmanaged_message(email)),
            _ => (
                SwitchReason::NoValidTarget,
                "there are no stored accounts; add one with `tagteam add`".into(),
            ),
        };
        Ok(noop(
            strategy_of(&req.target),
            reason,
            message,
            None,
            unmanaged,
        ))
    }

    /// §9: plan and ask the oracle without any lock; then take the mutation lock once, and
    /// under it the account locks and the live locks, and re-derive every decision. Anything
    /// that moved while this command waited releases every lock but the mutation lock and
    /// plans again, without the network, for at most `ATTEMPTS` lock acquisitions. A usage
    /// strategy first releases quarantines that no longer bind and collects (§9.3); the
    /// collector's warnings lead the outcome's.
    pub fn switch(&self, req: SwitchRequest) -> Result<SwitchOutcome, EngineError> {
        self.refuse_inside_run_shell()?;
        let provider = self.provider(&req.provider)?;
        let p = provider.as_ref();
        if !req.force {
            self.settle_or_refuse(&GuardRequest {
                source: req.source,
                ..GuardRequest::refusing_one(&req.provider)
            })?;
        }
        let Some(store) = self.existing_store()? else {
            return self.without_store(p, &req);
        };
        let mut warnings = self.prepare_usage(p, &store, &req)?;
        let mut outcome = self.switch_planned(p, &store, &req)?;
        warnings.append(&mut outcome.warnings);
        outcome.warnings = warnings;
        Ok(outcome)
    }

    /// `switch` from its first plan on.
    fn switch_planned(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
    ) -> Result<SwitchOutcome, EngineError> {
        // §7.2: before the mutation lock, the only place a manual switch may use the network.
        let mut plan = match self.plan(p, store, req, Ask::Oracle)? {
            Planned::Done(outcome) => return Ok(outcome),
            // §11.2 step 10: the tick has freshened an automatic switch's target by its own
            // table; freshening here again would act on the manual table instead.
            Planned::Go(plan) if req.auto.is_some() => plan,
            Planned::Go(plan) => match self.freshen_plan(p, store, req, plan)? {
                Planned::Done(outcome) => return Ok(outcome),
                Planned::Go(plan) => plan,
            },
        };
        // Once, before the mutation lock: a test callback here may take that lock itself.
        hooks::point(self, "planned")?;
        let guard = if req.force {
            self.mutation_guard()?
        } else {
            self.guard(&GuardRequest {
                source: req.source,
                ..GuardRequest::refusing_one(&req.provider)
            })?
            .0
        };
        for attempt in 1..=ATTEMPTS {
            if attempt > 1 {
                // No freshen here: this runs under the mutation lock, where no network is
                // allowed (§4.3). A pick that changed is activated with the vault's
                // generation, and CC refreshes it.
                let warnings = std::mem::take(&mut plan.warnings);
                plan = match self.plan(p, store, req, Ask::Reuse(plan.hint.take()))? {
                    Planned::Done(mut outcome) => {
                        outcome.warnings.extend(warnings);
                        return Ok(outcome);
                    }
                    Planned::Go(mut next) => {
                        next.warnings = warnings;
                        next
                    }
                };
            }
            let (_, outgoing) = self.live_row(p, store, &req.provider)?;
            let mut ids = vec![&plan.target.id];
            if let Some(o) = &outgoing {
                ids.push(&o.id);
            }
            let accounts = self.lock_accounts(&ids)?;
            let locks = p.lock_live(&self.env, &guard)?;
            match self.rederive(p, store, req, &plan, outgoing.as_ref())? {
                Rederived::Go(locked) => {
                    let ls = LockedSwitch {
                        guard: &guard,
                        store,
                        account_locks: &accounts,
                        locks: &locks,
                    };
                    let mut outcome = self.transact(p, &plan, locked, &ls, req)?;
                    drop(locks);
                    drop(accounts);
                    // §9.1 (amended): with CC's locks released, `oauthAccount` is read again.
                    if let Some(to) = &outcome.to {
                        if let Ok(identity) = p.parse_identity(&to.identity_json) {
                            outcome.warnings.extend(self.reverify_identity(
                                p,
                                &guard,
                                &req.provider,
                                &identity,
                            ));
                        }
                    }
                    // The re-plan needs no lock and never fetches (§8.3).
                    self.replan_polls(p, store, &outcome);
                    return Ok(outcome);
                }
                Rederived::Done(mut outcome) => {
                    outcome.warnings.extend(plan.warnings.clone());
                    return Ok(outcome);
                }
                Rederived::Replan => {}
            }
            // `locks`, then `accounts`, are released here; the mutation lock is kept.
        }
        Err(EngineError::LiveMoved)
    }

    /// §9.1 (amended): CC's start-up may write the global config without its lock for its first
    /// 30 s (Appendix A.7), so after a switch (or a recovery that activated) commits and CC's
    /// locks are released, the live identity is read again. If it is not the identity the
    /// switch wrote (by identity key, so a fuller `oauthAccount` of the same account stands), it
    /// is spliced once more under the config lock alone, with its own budget, and read a last
    /// time. A warning comes back only when the second read still differs or the splice failed:
    /// a starting Claude Code may have overwritten it, and switching again restores it. The
    /// switch has committed, so this is contained (§14): it never fails it. The log carries a
    /// fixed phrase, never an email or a label (§14.2).
    pub(crate) fn reverify_identity(
        &self,
        p: &dyn Provider,
        guard: &MutationGuard,
        provider: &ProviderId,
        written: &Identity,
    ) -> Option<String> {
        let key = p.identity_key(written);
        let intact = || match p.live_identity(&self.env) {
            Read::Present(live) => p.identity_key(&live) == key,
            Read::Absent | Read::Unreadable(_) => false,
        };
        let _ = hooks::point(self, "identity-reverify");
        if intact() {
            return None;
        }
        let spliced = p.resplice_identity(&self.env, guard, written);
        let _ = hooks::point(self, "identity-reverify");
        if spliced.is_ok() && intact() {
            tracing::info!(
                provider = self.registered_id(provider),
                "a starting {} had overwritten the switched account's identity, which was spliced again",
                p.display_name()
            );
            return None;
        }
        tracing::warn!(
            provider = self.registered_id(provider),
            "a starting {} may have overwritten the switched account's identity",
            p.display_name()
        );
        Some(format!(
            "a starting {} may have overwritten the account's identity in its config; run `tagteam switch` again to restore it",
            p.display_name()
        ))
    }

    /// §8.3: after a switch that activated an account, both accounts' polls are re-planned
    /// for their new roles, without fetching: the incoming account gets §9.4's plan (180 s
    /// after its reading, due at once when that is past) and the outgoing one the candidate
    /// policy (§8.6). The readings stay as they are. An account with no reading is skipped,
    /// so it keeps no plan and stays eligible on demand (§8.3: a plan in force would make it
    /// not due). The switch has committed, so this is contained (§14): a failure is logged at
    /// ERROR and never fails it.
    fn replan_polls(&self, p: &dyn Provider, store: &Store, outcome: &SwitchOutcome) {
        let Some(to) = &outcome.to else {
            return;
        };
        if !p.capabilities().usage {
            return;
        }
        let budget = p.poll_budget();
        let now_s = self.now_ms().div_euclid(1000);
        let outgoing = outcome.from.as_ref().filter(|from| from.id != to.id);
        for (row, active) in [(Some(to), true), (outgoing, false)] {
            let Some(row) = row else {
                continue;
            };
            let result = store.usage_state(&row.id).and_then(|state| {
                let Some(fetched_at) = state.and_then(|s| s.fetched_at) else {
                    return Ok(());
                };
                let plan = replan_for_role(&budget, active, fetched_at, now_s, jitter());
                store.set_poll_plan(&row.id, &plan)
            });
            if let Err(e) = result {
                tracing::error!(
                    position = row.position,
                    account = %row.id,
                    code = e.sqlite_code().map(tracing::field::debug),
                    "could not re-plan usage polls after the switch"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::PathBuf, sync::Arc};

    #[test]
    fn the_hint_matrix_is_the_one_the_renderer_used_to_hold() {
        let cc = tagteam_cc::ClaudeCode::new(
            Arc::new(tagteam_provider::FakeKeychain::new()),
            tagteam_provider::Platform::MacOs,
        );
        let w = cc.words();
        let stores = [
            SecretStore::Keychain,
            SecretStore::File(PathBuf::from("/c.json")),
            SecretStore::Fallback(PathBuf::from("/c.json")),
        ];
        for store in &stores {
            for (from, to) in [
                (Some("oauth"), "api_key"),
                (Some("api_key"), "oauth"),
                (Some("api_key"), "api_key"),
                (None, "api_key"),
                (Some("setup_token"), "api_key"),
            ] {
                assert_eq!(
                    activation_hint(&cc, from, to, store, false),
                    w.after_switch_managed_key,
                    "{from:?} -> {to} via {store:?}"
                );
            }
            assert_eq!(
                activation_hint(&cc, None, "oauth", store, true),
                w.after_switch_managed_key,
                "a live key no account holds, via {store:?}"
            );
        }
        assert_eq!(
            activation_hint(&cc, Some("oauth"), "setup_token", &stores[0], false),
            w.after_switch_keychain
        );
        assert_eq!(
            activation_hint(&cc, Some("oauth"), "oauth", &stores[1], false),
            w.after_switch_file
        );
        assert_eq!(
            activation_hint(&cc, Some("oauth"), "oauth", &stores[2], false),
            w.after_switch_file
        );
        assert_eq!(w.after_switch_managed_key, "Restart Claude Code to apply.");
        assert_eq!(w.after_switch_file, "Active on your next message.");
    }
}
