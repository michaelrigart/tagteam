use tagteam_core::autoswitch::Trigger;
use tagteam_core::fmt::span;
use tagteam_core::rank::{
    BestOrder, Candidate, NextAvailable, best_order, binding_window, blocked_until, next_available,
};
use tagteam_core::usage::headroom;
use tagteam_core::{AccountId, ProviderId, Window, rotation_order};
use tagteam_provider::{Credential, Provider};

use crate::engine::Engine;
use crate::error::{EngineError, SessionOwner};
use crate::store::{AccountRow, Store};

use super::plan::Plan;
use super::support::{Axis, OracleHint};
use super::{
    LIVE_CHANGED, LIVE_UNKNOWN, NO_LIVE, NO_VALID, ONE_UNRANKED, ONLY_ONE, SwitchOutcome,
    SwitchReason, SwitchRequest, SwitchTarget, USAGE_UNAVAILABLE, UsageStrategy, noop,
};

/// A rotation candidate by the store alone (§9.3 "Reading the vault lazily"): enabled, not
/// quarantined, and with an identity. Whether its vault holds a credential is read only when
/// the walk reaches it. `Engine::switch_candidate` adds the session rule, and every candidate
/// list goes through that instead.
pub(crate) fn is_candidate(row: &AccountRow) -> bool {
    !row.disabled && row.quarantine_reason.is_none() && row.identity_json.is_object()
}

/// A bare `switch` (§9.2, §9.3): where the rotation goes, or why it stays.
#[allow(clippy::large_enum_variant)]
pub(super) enum Rotation {
    /// The pick, and the accounts the walk read and passed over before it.
    To(AccountRow, Vec<AccountId>),
    Stay(SwitchReason, &'static str),
}

/// What a usage strategy decided (§9.3).
#[allow(clippy::large_enum_variant)]
pub(super) enum Ranked {
    /// The pick, and what the ranking tells the user.
    To(AccountRow, Vec<String>),
    /// A no-op: its reason, its message, and what the ranking tells the user.
    Stay(SwitchReason, String, Vec<String>),
}

/// A candidate as a usage strategy ranks it: its row and its decision-grade windows (`None`:
/// its reading cannot drive a decision, §8.4).
struct Rated {
    row: AccountRow,
    windows: Option<Vec<Window>>,
}

impl Rated {
    fn headroom(&self, models: &[String]) -> Option<f64> {
        self.windows.as_deref().and_then(|w| headroom(w, models))
    }

    fn candidate(&self, models: &[String]) -> Candidate {
        Candidate {
            position: self.row.position,
            headroom: self.headroom(models),
        }
    }

    /// Its binding window as a message names it: `7d at 77%`.
    fn binding(&self, models: &[String]) -> String {
        binding_text(self.windows.as_deref().unwrap_or_default(), models)
    }

    /// `a@x.co (7d at 77%)`.
    fn described(&self, models: &[String]) -> String {
        format!("{} ({})", self.row.label, self.binding(models))
    }
}

/// §8.2's binding window, as a strategy's message names it: its label and its pct, rounded as
/// `list` rounds it.
fn binding_text(windows: &[Window], models: &[String]) -> String {
    match binding_window(windows, models) {
        Some(w) => format!("{} at {}%", w.label, w.pct.round() as i64),
        None => "usage unknown".to_owned(),
    }
}

/// A walk that found no account to activate ends as a rotation's does (§9.3).
fn nothing_to_activate(live_row: Option<&AccountRow>) -> Ranked {
    match live_row {
        Some(_) => Ranked::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE.into(), Vec::new()),
        None => Ranked::Stay(SwitchReason::NoValidTarget, NO_VALID.into(), Vec::new()),
    }
}

impl Engine {
    /// §9.3: a candidate for every strategy. `is_candidate`, and not session-owned (§12.5): a
    /// live reservation, or a live or unreadable session record. The vault is not read.
    pub(crate) fn switch_candidate(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<bool, EngineError> {
        Ok(is_candidate(row) && !self.session_state(p, row)?.owned())
    }

    /// §9.3: the rotation's candidates in walk order, from the store and each account's session
    /// state (`switch_candidate`). `None` when the live anchor is managed and fewer than two
    /// accounts qualify (§9.2). The walk starts after the live account when it is managed
    /// (`live_row`), even if the store's active account disagrees (§6.1: the live identity
    /// wins). With no live login, or an unmanaged one, it starts at the store's active account
    /// if that is a candidate, then goes on from the first position.
    fn candidate_order(
        &self,
        p: &dyn Provider,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
    ) -> Result<Option<Vec<AccountRow>>, EngineError> {
        let mut candidates = Vec::new();
        for row in store.accounts(provider)? {
            if self.switch_candidate(p, &row)? {
                candidates.push(row);
            }
        }
        if live_row.is_some() && candidates.len() < 2 {
            return Ok(None);
        }
        let positions: Vec<u32> = candidates.iter().map(|a| a.position).collect();
        let order: Vec<u32> = match live_row {
            Some(live) => rotation_order(&positions, Some(live.position)),
            None => {
                let rest = rotation_order(&positions, None);
                let active = store
                    .active(provider)?
                    .and_then(|id| candidates.iter().find(|a| a.id == id))
                    .map(|a| a.position);
                match active {
                    Some(first) => std::iter::once(first)
                        .chain(rest.into_iter().filter(|p| *p != first))
                        .collect(),
                    None => rest,
                }
            }
        };
        Ok(Some(
            order
                .into_iter()
                .filter_map(|pos| candidates.iter().find(|a| a.position == pos).cloned())
                .collect(),
        ))
    }

    /// §9.3's lazy walk, shared by rotation and the usage strategies: reads each vault in
    /// `order` only until one holds a credential, and returns that pick with the accounts it
    /// passed over. An unreadable vault before the pick could have been the pick, so the walk
    /// fails naming the account; no vault after the pick is read.
    fn walk(
        &self,
        order: impl IntoIterator<Item = AccountRow>,
    ) -> Result<Option<(AccountRow, Vec<AccountId>)>, EngineError> {
        let mut walked = Vec::new();
        for row in order {
            if self.vault_holds_login(&row)? {
                return Ok(Some((row, walked)));
            }
            walked.push(row.id);
        }
        Ok(None)
    }

    /// §9.3 rotation, reading the vault lazily (`walk`). The candidates are counted from the
    /// store and their session state: with a managed live anchor and fewer than two of them,
    /// it stays put (§9.2).
    pub(super) fn rotation(
        &self,
        p: &dyn Provider,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
    ) -> Result<Rotation, EngineError> {
        let Some(order) = self.candidate_order(p, store, provider, live_row)? else {
            return Ok(Rotation::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE));
        };
        if let Some((row, walked)) = self.walk(order)? {
            return Ok(Rotation::To(row, walked));
        }
        Ok(match live_row {
            Some(_) => Rotation::Stay(SwitchReason::OnlyOneAccount, ONLY_ONE),
            None => Rotation::Stay(SwitchReason::NoValidTarget, NO_VALID),
        })
    }

    /// Under the locks: whether the plan's rotation pick still stands, decided from the store
    /// and session state alone (§9.3 "under the locks only the chosen account is read again").
    /// It does when the pick is still a candidate and every candidate the new walk order puts
    /// before it is one the planning walk already read and passed over.
    pub(super) fn rotation_pick_stands(
        &self,
        p: &dyn Provider,
        store: &Store,
        plan: &Plan,
        anchor: Option<&AccountRow>,
    ) -> Result<bool, EngineError> {
        let Some(order) = self.candidate_order(p, store, &plan.target.provider, anchor)? else {
            return Ok(false);
        };
        Ok(order
            .iter()
            .position(|r| r.id == plan.target.id)
            .is_some_and(|at| order[..at].iter().all(|r| plan.walked.contains(&r.id))))
    }

    /// §9.3's usage strategies, planned from the store and session state: the candidates are a
    /// rotation's (`candidate_order`), each ranked by its decision-grade headroom under
    /// `models` (§8.2, §8.4), and their vaults are read lazily in the strategy's own order
    /// (`walk`).
    pub(super) fn usage_pick(
        &self,
        p: &dyn Provider,
        store: &Store,
        provider: &ProviderId,
        live_row: Option<&AccountRow>,
        strategy: UsageStrategy,
        models: &[String],
    ) -> Result<Ranked, EngineError> {
        let Some(order) = self.candidate_order(p, store, provider, live_row)? else {
            return Ok(Ranked::Stay(
                SwitchReason::OnlyOneAccount,
                ONLY_ONE.into(),
                Vec::new(),
            ));
        };
        // Only without a managed live login: nothing in the store can be a candidate.
        if order.is_empty() {
            return Ok(Ranked::Stay(
                SwitchReason::NoValidTarget,
                NO_VALID.into(),
                Vec::new(),
            ));
        }
        let rated = order
            .into_iter()
            .map(|row| {
                Ok(Rated {
                    windows: self.decision_windows(&row, models)?,
                    row,
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        match strategy {
            UsageStrategy::Best => self.best_pick(&rated, live_row, models),
            UsageStrategy::NextAvailable => self.next_available_pick(&rated, live_row, models),
        }
    }

    /// §9.3 `best`: the known candidate with the most headroom, if it beats the live
    /// account's; every known one, with a warning, when the live headroom is unknown or there
    /// is no managed live login, but with a managed live login of unknown headroom never one
    /// known to be at its limit (`candidates-exhausted` when all are). A candidate whose
    /// headroom is unknown is never picked, and a warning counts them. A better candidate whose vault holds nothing is not switchable, so
    /// a walk that finds none to activate is `already-best`; with the live headroom unknown, it
    /// is `candidates-exhausted` when candidates known to be at their limit were left out, and
    /// `usage-unavailable` otherwise.
    fn best_pick(
        &self,
        rated: &[Rated],
        live_row: Option<&AccountRow>,
        models: &[String],
    ) -> Result<Ranked, EngineError> {
        let candidates: Vec<Candidate> = rated.iter().map(|r| r.candidate(models)).collect();
        let at = |position: &u32| rated.iter().find(|r| r.row.position == *position);
        let live = match live_row {
            Some(row) => Some(Rated {
                windows: self.decision_windows(row, models)?,
                row: row.clone(),
            }),
            None => None,
        };
        let live_headroom = live.as_ref().and_then(|l| l.headroom(models));
        let mut notes = match candidates.iter().filter(|c| c.headroom.is_none()).count() {
            0 => Vec::new(),
            1 => vec![ONE_UNRANKED.to_owned()],
            n => vec![format!(
                "{n} candidates have no usage reading recent enough to rank by; they were not considered"
            )],
        };
        Ok(match best_order(live_headroom, &candidates) {
            BestOrder::UsageUnavailable => Ranked::Stay(
                SwitchReason::UsageUnavailable,
                USAGE_UNAVAILABLE.into(),
                Vec::new(),
            ),
            BestOrder::AlreadyBest => {
                let leader = match best_order(None, &candidates) {
                    BestOrder::Try(order) => order.first().and_then(at),
                    _ => None,
                };
                let message = match (&live, leader) {
                    (Some(live), Some(leader)) => format!(
                        "{} already has the most headroom ({}); the best candidate is {}",
                        live.row.label,
                        live.binding(models),
                        leader.described(models)
                    ),
                    _ => "the live account already has the most headroom".to_owned(),
                };
                Ranked::Stay(SwitchReason::AlreadyBest, message, notes)
            }
            BestOrder::Try(order) => {
                // §9.3: a managed live login of unknown usage is switched away from, but never
                // to a candidate known to be at its limit. With no live login at all, the best
                // known is picked as it is.
                let (order, exhausted): (Vec<u32>, Vec<u32>) =
                    if live_row.is_some() && live_headroom.is_none() {
                        order.into_iter().partition(|position| {
                            at(position)
                                .and_then(|r| r.headroom(models))
                                .is_none_or(|h| h > 0.0)
                        })
                    } else {
                        (order, Vec::new())
                    };
                if order.is_empty() {
                    let exhausted: Vec<&Rated> = exhausted.iter().filter_map(at).collect();
                    return Ok(Ranked::Stay(
                        SwitchReason::CandidatesExhausted,
                        self.exhausted_message(&exhausted, models),
                        notes,
                    ));
                }
                match self.walk(order.iter().filter_map(at).map(|r| r.row.clone()))? {
                    Some((pick, _)) => {
                        if live_headroom.is_none() {
                            let why = if live_row.is_some() {
                                LIVE_UNKNOWN
                            } else {
                                NO_LIVE
                            };
                            notes.push(why.to_owned());
                        }
                        Ranked::To(pick, notes)
                    }
                    None => match &live {
                        Some(live) if live_headroom.is_some() => Ranked::Stay(
                            SwitchReason::AlreadyBest,
                            format!(
                                "no candidate with more headroom than {} holds a stored credential",
                                live.described(models)
                            ),
                            notes,
                        ),
                        // §9.3: no candidate that beat the exhausted ones holds a stored
                        // credential, so none of them is switchable; what is left is known to
                        // be at its limit, as `next-available` reports it.
                        _ if !exhausted.is_empty() => {
                            let exhausted: Vec<&Rated> = exhausted.iter().filter_map(at).collect();
                            Ranked::Stay(
                                SwitchReason::CandidatesExhausted,
                                self.exhausted_message(&exhausted, models),
                                notes,
                            )
                        }
                        _ => Ranked::Stay(
                            SwitchReason::UsageUnavailable,
                            "no candidate with a known usage reading holds a stored credential"
                                .into(),
                            notes,
                        ),
                    },
                }
            }
        })
    }

    /// §9.3 `next-available`: the rotation's walk without the candidates known to be at their
    /// limit (§8.2: unknown headroom is never skipped). Each skipped account is named, with its
    /// binding window, in a warning. If every candidate is skipped, `candidates-exhausted`
    /// names them all and when the first of those windows resets.
    fn next_available_pick(
        &self,
        rated: &[Rated],
        live_row: Option<&AccountRow>,
        models: &[String],
    ) -> Result<Ranked, EngineError> {
        let walk: Vec<Candidate> = rated.iter().map(|r| r.candidate(models)).collect();
        let at = |position: &u32| rated.iter().find(|r| r.row.position == *position);
        let (order, skipped) = match next_available(&walk) {
            NextAvailable::Exhausted => {
                let all: Vec<&Rated> = rated.iter().collect();
                return Ok(Ranked::Stay(
                    SwitchReason::CandidatesExhausted,
                    self.exhausted_message(&all, models),
                    Vec::new(),
                ));
            }
            NextAvailable::Try { order, skipped } => (order, skipped),
        };
        let skipped: Vec<&Rated> = skipped.iter().filter_map(at).collect();
        let notes = skipped
            .iter()
            .map(|r| {
                format!(
                    "skipped {} (position {}): at its limit ({})",
                    r.row.label,
                    r.row.position,
                    r.binding(models)
                )
            })
            .collect();
        Ok(
            match self.walk(order.iter().filter_map(at).map(|r| r.row.clone()))? {
                Some((pick, _)) => Ranked::To(pick, notes),
                None if skipped.is_empty() => nothing_to_activate(live_row),
                // §9.3: an account whose vault holds nothing is not switchable, so every
                // switchable candidate was skipped.
                None => Ranked::Stay(
                    SwitchReason::CandidatesExhausted,
                    self.exhausted_message(&skipped, models),
                    Vec::new(),
                ),
            },
        )
    }

    /// `candidates-exhausted`'s message: each candidate with its binding window, then how long
    /// until the first of them is usable again (§9.3; §11.2 step 8: the binding window first,
    /// then its reset). A candidate is back once every relevant window at its limit has reset,
    /// so its time is the latest of those resets, and the message names the earliest candidate.
    /// No reset is named when none of them has one.
    fn exhausted_message(&self, rated: &[&Rated], models: &[String]) -> String {
        let named: Vec<String> = rated.iter().map(|r| r.described(models)).collect();
        let reset = rated
            .iter()
            .filter_map(|r| blocked_until(r.windows.as_deref().unwrap_or_default(), models))
            .min();
        let now_s = self.now_ms().div_euclid(1000);
        let when = reset.map_or_else(String::new, |at| {
            format!("; the earliest reset is in {}", span(at - now_s))
        });
        format!(
            "every candidate is at its limit: {}{when}",
            named.join(", ")
        )
    }

    /// §9.4 "Before locking": asks the oracle about the outgoing live secret when it is not
    /// that account's vault generation. Never called under the mutation lock.
    pub(super) fn oracle_hint(
        &self,
        p: &dyn Provider,
        out: &AccountRow,
    ) -> Result<Option<OracleHint>, EngineError> {
        let Some(bytes) = Axis::of(p, &out.kind).live_secret(&p.read_live_auth(&self.env)) else {
            return Ok(None);
        };
        if bytes.is_empty() || self.matches_vault(p, out, &bytes) {
            return Ok(None);
        }
        // §14.1: planning is before any lock, and a signal that has landed ends it here.
        self.check_cancel()?;
        let resolved = self.oracle.resolve(p, &Credential::fresh(bytes.clone()));
        Ok(Some(OracleHint { bytes, resolved }))
    }

    /// §11.2 step 11: an automatic switch's preconditions, in the spec's order. `None` when
    /// every one holds, or for any other switch; otherwise the no-op that says which failed.
    /// `live_row` is the live account as just read. Planning runs it first, so a refused switch
    /// spends no oracle request; `rederive` runs it again under every lock, where it decides
    /// (§9.4 step 1). Nothing is written either way.
    pub(super) fn auto_refusal(
        &self,
        p: &dyn Provider,
        store: &Store,
        req: &SwitchRequest,
        live_row: Option<&AccountRow>,
    ) -> Result<Option<SwitchOutcome>, EngineError> {
        let Some(auto) = &req.auto else {
            return Ok(None);
        };
        let SwitchTarget::Account(id) = &req.target else {
            return Err(EngineError::InvalidInput(
                "an automatic switch names its target".into(),
            ));
        };
        let refuse = |reason: SwitchReason, message: String| {
            Ok(Some(noop(
                "direct",
                reason,
                message,
                live_row.cloned(),
                None,
            )))
        };
        // A manual switch made meanwhile, a login made in Claude Code, or a logout.
        if live_row.map(|r| &r.id) != Some(&auto.expected_from) {
            return refuse(SwitchReason::LiveChanged, LIVE_CHANGED.into());
        }
        // §11.2 step 6, judged from the state as read now: the engine is its one writer, and
        // the check and the record are both made under `MutationGuard`.
        if matches!(auto.trigger, Trigger::Proactive | Trigger::ConsumeFirst) {
            let now_s = self.now_ms().div_euclid(1000);
            if let Some(at) = store.autoswitch_state(&req.provider)?.last_switch_at {
                let ends = at.saturating_add(auto.cooldown_s);
                if now_s < ends {
                    return refuse(
                        SwitchReason::Cooldown,
                        format!(
                            "the cooldown after the last automatic switch has {} left",
                            span(ends - now_s)
                        ),
                    );
                }
            }
        }
        // §11.2 step 7: switchable (a vault credential and an identity, not disabled), not
        // quarantined, not session-owned (§12.5). A session that took the target is a
        // `not-candidate` no-op here, never §9.2's `session-owned` error, so the tick moves on
        // to its next target; under the locks this answer stands until they are released.
        let Some(target) = store.account(id)?.filter(|a| a.provider == req.provider) else {
            return refuse(
                SwitchReason::NotCandidate,
                "the account to switch to was removed".into(),
            );
        };
        let state = self.session_state(p, &target)?;
        let why = if target.disabled {
            Some("it is disabled".to_owned())
        } else if target.quarantine_reason.is_some() {
            Some("it needs a new login".to_owned())
        } else if state.owned() {
            // The same words as `session-owned`'s refusal: every owner and every file.
            Some(SessionOwner::of(&state, p).reason("it"))
        } else if !self.has_login(&target)? {
            Some("it has no stored credential".to_owned())
        } else {
            None
        };
        match why {
            Some(why) => refuse(
                SwitchReason::NotCandidate,
                format!(
                    "{} (position {}) is no longer a candidate: {why}",
                    target.label, target.position
                ),
            ),
            None => Ok(None),
        }
    }
}
