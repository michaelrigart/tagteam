//! §11.1 and §11.2: the auto-switch engine for one provider. It holds the provider's engine
//! lock for its whole life and runs one tick at a time; the loop that schedules its ticks
//! belongs to its caller (Decision 7).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use tagteam_core::autoswitch::{
    AccountSnapshot, AutoConfig, AutoState, Decision, Live, NoSwitchReason, Outcome, Phase,
    Snapshot, Strategy, Trigger, decide, departure,
};
use tagteam_core::rank::span;
use tagteam_core::usage::headroom;
use tagteam_core::{AccountId, Fingerprint, ProviderId, Window, WindowKind};
use tagteam_provider::{Env, FlockGuard, LockError, ProcessStamp, Provider, Read, ReadError};

use crate::collect::{CollectMode, CollectReport, Collected};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{AccountRow, Store, backoff_holds};
use crate::switch::{AutoFreshened, AutoPerform, SwitchReason, SwitchRequest, SwitchTarget};

/// Where a tick's events go (Decision 6). The CLI renders them as human lines or as JSONL.
pub trait EventSink {
    fn emit(&self, e: &AutoEvent);
    /// The end of a tick of `provider`'s engine: `AutoEngine::tick` calls it exactly once, last,
    /// whatever the tick came to, an `Err` too. A sink that keeps something for the rest of a
    /// tick (the human line's poll) lets it go here.
    fn tick_done(&self, _provider: &ProviderId) {}
}

/// §11.4's events. Every one carries its provider. An account is named by its position, and
/// by its email (its label, for a provider whose logins have none).
#[derive(Debug, Clone, PartialEq)]
pub enum AutoEvent {
    /// After step 3's collection: the live account, every account's decision-grade headroom
    /// by position (`None`: unknown), the threshold, each failed fetch's `last_error` token by
    /// position, and each decision-grade reading's windows by position (key → pct).
    Poll {
        provider: ProviderId,
        active: Option<(u32, String)>,
        headroom_pct: BTreeMap<u32, Option<f64>>,
        threshold: f64,
        fetch_errors: BTreeMap<u32, String>,
        windows_pct: BTreeMap<u32, BTreeMap<String, f64>>,
    },
    /// A switch made, or in dry-run one that would have been made (§11.2 step 10).
    Switch {
        provider: ProviderId,
        trigger: Trigger,
        from: u32,
        to: u32,
        warnings: Vec<String>,
        dry_run: bool,
    },
    /// `reason` is a `NoSwitchReason` in kebab-case; `detail` as `Decision::NoSwitch` has it.
    NoSwitch {
        provider: ProviderId,
        reason: String,
        detail: String,
    },
    /// `reason` is the stored `quarantine_reason` (§6.1).
    AccountQuarantined {
        provider: ProviderId,
        number: u32,
        email: String,
        reason: String,
    },
    /// `reason` is `account-replaced` or `credentials-replaced` (§11.4).
    AccountUnquarantined {
        provider: ProviderId,
        number: u32,
        email: String,
        reason: String,
    },
    /// After `no-switch all-exhausted`: the earliest recovery, epoch seconds.
    AllExhausted {
        provider: ProviderId,
        earliest_reset_at: Option<i64>,
    },
    /// The loop's, before a long sleep (§11.4); a tick never emits it.
    Sleep {
        provider: ProviderId,
        seconds: f64,
        until: i64,
    },
    Error {
        provider: ProviderId,
        message: String,
        transient: bool,
    },
    ConfigWarning {
        provider: ProviderId,
        message: String,
    },
}

/// What one tick came to, for `--once`'s exit code (§11.1, §11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    Switched,
    NoAction,
    Blocked,
    Error,
}

impl TickOutcome {
    fn of(d: &Decision) -> Self {
        match d {
            Decision::Switch { .. } => TickOutcome::Switched,
            Decision::NoSwitch {
                outcome: Outcome::NoAction,
                ..
            } => TickOutcome::NoAction,
            Decision::NoSwitch {
                outcome: Outcome::Blocked,
                ..
            } => TickOutcome::Blocked,
        }
    }
}

/// An account's quarantine as a tick last saw it, with its login epoch (Decision 10).
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    reason: Option<String>,
    login_epoch: i64,
}

/// One account as a tick read it from the store: its row, its decision-grade windows under
/// the tick's models (§8.4), when its reading was taken, as stored, and whether a session
/// owns it (§12.5), read beside them.
struct Account {
    row: AccountRow,
    windows: Option<Vec<Window>>,
    fetched_at: Option<i64>,
    session_owned: bool,
}

/// The auto-switch engine of one provider (§11.1).
pub struct AutoEngine<'e> {
    engine: &'e Engine,
    provider: ProviderId,
    cfg: AutoConfig,
    dry_run: bool,
    /// The engine lock, held for this engine's life; `None` in dry-run.
    _lock: Option<FlockGuard>,
    /// Dry-run's `unhealthy_ticks`: it writes no auto-switch state (§11.1).
    unhealthy: Option<u32>,
    /// Decision 10: each account's quarantine as the previous tick left it.
    seen: Option<BTreeMap<AccountId, Seen>>,
    /// The `models` the model-name check last ran for (§11.2 step 3).
    checked_models: Option<Vec<String>>,
    /// The strategy the long-window check last ran for (§4.5).
    checked_strategy: Option<Strategy>,
    /// The live account whose ticks `unhealthy_ticks` last counted (Decision 4).
    judged: Option<AccountId>,
    /// The managed live account as the last tick left it.
    live: Option<AccountId>,
}

/// Replaces the held engine lock file's whole content with this process's pid and start time,
/// one JSON line, through a second descriptor: the lock stays on the first. M5's `doctor` reads
/// it; nothing reads it for exclusion.
fn write_holder(path: &Path) -> Result<(), EngineError> {
    let stamp = ProcessStamp::current()?;
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.set_len(0)?;
    file.write_all(format!("{{\"pid\":{},\"start\":{}}}\n", stamp.pid, stamp.start).as_bytes())?;
    Ok(())
}

/// §5: `locks/autoswitch-<provider>.lock`, the provider's engine lock (§11.1). `auto` holds it
/// for an engine's life, and `purge` while it runs (§10.5 step 3).
///
/// A provider ID is a string the store holds, so one that is not a plain name (anything outside
/// `[A-Za-z0-9._-]`, or one starting with `.`) cannot name a path: its lock is
/// `locks/engine-<first 12 hex digits of the ID's SHA-256>.lock`, inside `locks/`. Every
/// registered provider's ID is a plain name and keeps its `autoswitch-` file.
pub(crate) fn engine_lock_path(env: &Env, provider: &ProviderId) -> PathBuf {
    let id = provider.as_str();
    let plain = !id.is_empty()
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    let name = if plain {
        format!("autoswitch-{id}.lock")
    } else {
        format!(
            "engine-{}.lock",
            Fingerprint::of_secret(id.as_bytes()).short12()
        )
    };
    env.data_dir().join("locks").join(name)
}

/// The holder `write_holder` recorded in an engine lock file (§11.1), read without trying the
/// lock, which an `auto` starting at that instant would then find taken (§13.6): the holder
/// an engine named, never proof that it still holds the lock. `Absent` when
/// there is no file, or an empty one: no engine has written its record there yet.
pub(crate) fn read_holder(path: &Path) -> Read<ProcessStamp> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Read::Absent,
        Err(e) => {
            return Read::Unreadable(ReadError::new(path.display().to_string(), e.to_string()));
        }
    };
    if bytes.is_empty() {
        return Read::Absent;
    }
    let v: Option<serde_json::Value> = serde_json::from_slice(&bytes).ok();
    let pid = v
        .as_ref()
        .and_then(|v| v["pid"].as_u64())
        .and_then(|p| u32::try_from(p).ok());
    let start = v.as_ref().and_then(|v| v["start"].as_u64());
    match (pid, start) {
        (Some(pid), Some(start)) => Read::Present(ProcessStamp { pid, start }),
        _ => Read::Unreadable(ReadError::new(
            path.display().to_string(),
            "it is not a pid and a start time",
        )),
    }
}

impl Engine {
    /// §11.1: the auto-switch engine for `provider`. It tries the provider's engine lock,
    /// `locks/autoswitch-<provider>.lock`, once and never waits: `Ok(None)` while another
    /// process holds it (`engine-running`). M5's amendment (`86882e3`): "Once it holds the
    /// lock, the engine writes its pid and start time into the lock file, the start time taken
    /// as for the `switch_journal` holder (§12.6). The record is never used for exclusion."
    /// Dry-run takes no lock and writes nothing here. Like every command that changes the live
    /// login, a real engine refuses inside a `tagteam run` shell; dry-run does not (§11.1).
    pub fn auto(
        &self,
        provider: &ProviderId,
        cfg: AutoConfig,
        dry_run: bool,
    ) -> Result<Option<AutoEngine<'_>>, EngineError> {
        self.provider(provider)?;
        let lock = if dry_run {
            None
        } else {
            self.refuse_inside_run_shell()?;
            let path = engine_lock_path(&self.env, provider);
            let Some(lock) = FlockGuard::try_lock(&path)? else {
                return Ok(None);
            };
            write_holder(lock.path())?;
            Some(lock)
        };
        Ok(Some(AutoEngine {
            engine: self,
            provider: provider.clone(),
            cfg,
            dry_run,
            _lock: lock,
            unhealthy: None,
            seen: None,
            checked_models: None,
            checked_strategy: None,
            judged: None,
            live: None,
        }))
    }
}

/// An interruption, or a store that failed: a tick returns these as `Err`.
fn fatal(e: &EngineError) -> bool {
    e.signal().is_some() || matches!(e, EngineError::Store(_))
}

/// An error a retry at the next tick may clear by itself.
fn transient(e: &EngineError) -> bool {
    e.kind() == "lock-timeout"
}

/// The decision an `Error` tick returns when it failed before deciding: NO_ACTION, so a loop
/// keeps its normal cadence (§11.4). Its `error` event is the report; this is never rendered.
fn undecided(detail: String) -> Decision {
    Decision::NoSwitch {
        reason: NoSwitchReason::NoActiveAccount,
        outcome: Outcome::NoAction,
        detail,
        earliest_reset: None,
    }
}

/// How events name an account: its email, or its label when its provider's logins have none.
fn email_of(row: &AccountRow) -> String {
    row.email.clone().unwrap_or_else(|| row.label.clone())
}

impl AutoEngine<'_> {
    /// One §11.2 tick. `Err` only for an interruption (§14.1) or a store failure; every other
    /// failure is an `error` event and `TickOutcome::Error`. A switch that committed is
    /// `Switched` even when a signal arrived during its critical span: the token stays set for
    /// the caller's next cancellation point. Every tick ends with `sink.tick_done`.
    pub fn tick(&mut self, sink: &dyn EventSink) -> Result<(TickOutcome, Decision), EngineError> {
        let ticked = match self.run(sink) {
            Err(e) if !fatal(&e) => {
                self.error(sink, e.to_string(), transient(&e));
                Ok((TickOutcome::Error, undecided(e.to_string())))
            }
            done => done,
        };
        sink.tick_done(&self.provider);
        ticked
    }

    /// A settings reload (Decision 9): the next tick decides and collects with `cfg`, and runs
    /// the model-name check again when `models` changed.
    pub fn set_config(&mut self, cfg: AutoConfig) {
        self.cfg = cfg;
    }

    /// When the live account can next be fetched: the latest of its planned poll, its usage
    /// lease's expiry and its failure backoff, since a lease outlives its record (§8.3) and a
    /// tick that wakes inside any of them cannot fetch. A backoff that no legal schedule
    /// reaches is ignored, as reserving ignores it (§8.4). `None` before a tick has seen a
    /// managed live account, or when it has no plan; a store that cannot be read is `None`
    /// too.
    pub fn active_next_poll_at(&self) -> Option<i64> {
        let id = self.live.as_ref()?;
        let store = self.engine.existing_store().ok()??;
        let state = store.usage_state(id).ok()??;
        let mut at = state.next_poll_at?;
        if let Some(ms) = store.usage_lease_expires_at(id).ok().flatten() {
            at = at.max((ms + 999).div_euclid(1000));
        }
        let now = self.engine.now_ms().div_euclid(1000);
        if let Some(until) = state.backoff_until.filter(|u| backoff_holds(Some(*u), now)) {
            at = at.max(until);
        }
        Some(at)
    }

    fn run(&mut self, sink: &dyn EventSink) -> Result<(TickOutcome, Decision), EngineError> {
        // §14.1: between ticks.
        self.engine.check_cancel()?;
        let p = self.engine.provider(&self.provider)?;
        let p = p.as_ref();
        // Step 1. Dry-run releases nothing (§11.1).
        let released = if self.dry_run {
            Vec::new()
        } else {
            self.engine
                .release_unbound_quarantines(&self.provider, "auto")?
        };
        let store = self.engine.existing_store()?;
        let rows = match &store {
            Some(s) => s.accounts(&self.provider)?,
            None => Vec::new(),
        };
        self.report_quarantines(&rows, &released, sink);
        // Step 2: recovery first, then the live check.
        if let Some(refusal) = self.recover()? {
            return Ok(self.no_switch(
                sink,
                NoSwitchReason::InterruptedSwitch,
                refusal.to_string(),
            ));
        }
        let live = match self.engine.read_live_identity(p)? {
            None => None,
            Some(identity) => {
                let key = p.identity_key(&identity);
                match rows.iter().find(|r| r.identity_key == key.as_str()) {
                    Some(row) => Some(row.clone()),
                    None => {
                        self.live = None;
                        return Ok(self.no_switch(
                            sink,
                            NoSwitchReason::UnmanagedActiveAccount,
                            String::new(),
                        ));
                    }
                }
            }
        };
        let (Some(store), Some(live)) = (store, live) else {
            self.live = None;
            return Ok(self.no_switch(sink, NoSwitchReason::NoActiveAccount, String::new()));
        };
        self.live = Some(live.id.clone());
        // Step 3.
        let report = self.engine.collect_usage(CollectMode::Scheduled {
            provider: self.provider.clone(),
            threshold: self.cfg.threshold,
            models: self.cfg.models.clone(),
        })?;
        let mut accounts = self.read(p, &store)?;
        self.report_collection(&report, &accounts, sink);
        self.poll(&report, &accounts, &live, sink);
        self.check_settings(p, &store, &accounts, sink)?;
        // Steps 4 to 9.
        let (state, stored) = self.state(&store, &live.id)?;
        let mut snap = self.snapshot(p, &accounts, &live.id);
        let mut decided = decide(&snap, &state, &self.cfg, Phase::Initial);
        if let Decision::Switch { recheck: true, .. } = &decided.decision {
            // Decision 2: re-check the current account and every candidate, then decide again
            // from what the store holds now, with the same state. The re-check skips a reading
            // at most 180 s old when it begins, so the decision judges freshness from then:
            // the time the re-check itself takes must not make that reading stale (§8.3).
            let began = snap.now;
            let report = self.engine.collect_usage(CollectMode::Recheck {
                accounts: recheck_ids(&snap, &live.id),
                threshold: self.cfg.threshold,
                models: self.cfg.models.clone(),
            })?;
            accounts = self.read(p, &store)?;
            self.report_collection(&report, &accounts, sink);
            snap = Snapshot {
                now: began,
                ..self.snapshot(p, &accounts, &live.id)
            };
            decided = decide(&snap, &state, &self.cfg, Phase::Rechecked);
        }
        self.judged = Some(live.id.clone());
        let ticked = Ticked {
            store: &store,
            live: &live,
            snap,
            stored,
            unhealthy: decided.unhealthy_ticks,
        };
        match decided.decision {
            Decision::Switch {
                trigger,
                ref targets,
                ..
            } => self.perform(p, &ticked, trigger, targets, &decided.decision, sink),
            decision => {
                self.count(&ticked)?;
                self.announce(sink, &decision);
                Ok((TickOutcome::of(&decision), decision))
            }
        }
    }

    /// Steps 10 to 12 for a `Switch` decision: each target in order, freshened (§7.2) and
    /// then switched to through the ordinary transaction, with the tick's preconditions.
    fn perform(
        &mut self,
        p: &dyn Provider,
        t: &Ticked<'_>,
        trigger: Trigger,
        targets: &[AccountId],
        decision: &Decision,
        sink: &dyn EventSink,
    ) -> Result<(TickOutcome, Decision), EngineError> {
        let mut failed = Vec::new();
        for id in targets {
            let Some(row) = t.store.account(id)? else {
                continue;
            };
            if self.dry_run {
                // §11.1: it freshens and switches nothing.
                self.count(t)?;
                self.emit(sink, |provider| AutoEvent::Switch {
                    provider,
                    trigger,
                    from: t.live.position,
                    to: row.position,
                    warnings: Vec::new(),
                    dry_run: true,
                });
                return Ok((TickOutcome::Switched, decision.clone()));
            }
            let fresh = match self.engine.freshen_auto(p, &row) {
                Ok(fresh) => fresh,
                Err(e) if fatal(&e) => return Err(e),
                Err(e) => AutoFreshened::Failed(e.to_string()),
            };
            match fresh {
                AutoFreshened::Ready => {}
                AutoFreshened::Quarantined => {
                    if let Some(now) = t.store.account(id)? {
                        self.note_quarantined(&now, sink);
                    }
                    continue;
                }
                AutoFreshened::Failed(why) => {
                    failed.push(format!("{} (position {}): {why}", row.label, row.position));
                    continue;
                }
                AutoFreshened::Skip => continue,
            }
            // Step 11.
            let req = SwitchRequest {
                provider: self.provider.clone(),
                target: SwitchTarget::Account(id.clone()),
                force: false,
                source: "auto",
                auto: Some(AutoPerform {
                    expected_from: t.live.id.clone(),
                    trigger,
                    cooldown_s: self.cfg.cooldown_s,
                    departure: departure(&t.snap, &self.cfg, &t.live.id, trigger),
                }),
            };
            match self.engine.switch(req) {
                Ok(out) if out.switched => {
                    // The commit reset `unhealthy_ticks` (Decision 4): nothing to count.
                    self.live = Some(id.clone());
                    self.judged = Some(id.clone());
                    self.emit(sink, |provider| AutoEvent::Switch {
                        provider,
                        trigger,
                        from: t.live.position,
                        to: row.position,
                        warnings: out.warnings,
                        dry_run: false,
                    });
                    return Ok((TickOutcome::Switched, decision.clone()));
                }
                Ok(out) => match out.reason {
                    SwitchReason::NotCandidate => continue,
                    SwitchReason::Cooldown => {
                        self.count(t)?;
                        let left = self.cooldown_left(t.store)?;
                        return Ok(self.no_switch(sink, NoSwitchReason::Cooldown, left));
                    }
                    SwitchReason::LiveChanged => {
                        // The count belonged to the account this tick judged, which is no
                        // longer live: the new live account starts from 0 (Decision 4).
                        if t.stored != 0 {
                            t.store.set_unhealthy_ticks(&self.provider, 0)?;
                        }
                        return Ok(self.no_switch(
                            sink,
                            NoSwitchReason::LiveChanged,
                            String::new(),
                        ));
                    }
                    // No other no-op answers a direct switch to an account other than the live
                    // one; should one ever, it is reported, never taken for a switch.
                    _ => {
                        self.count(t)?;
                        self.error(sink, out.message, false);
                        return Ok((TickOutcome::Error, decision.clone()));
                    }
                },
                Err(e) if fatal(&e) => return Err(e),
                // §12.5: the target's quiescent profile and its vault both moved, so nothing of
                // it can be activated until an explicit replacement resolves it. Freshening
                // finds a conflict that is already there (`Skip`); this is one that arose since,
                // found by the switch's own lazy capture under its locks. Either way it is
                // passed over for the next target, never an error that ends the tick.
                Err(EngineError::ProfileConflict { .. }) => {
                    tracing::warn!(
                        position = row.position,
                        account = %row.id,
                        "the target's session profile conflicts with its vault; trying the next target"
                    );
                    continue;
                }
                Err(e) => {
                    self.count(t)?;
                    self.error(
                        sink,
                        format!(
                            "could not switch to {} (position {}): {e}",
                            row.label, row.position
                        ),
                        transient(&e),
                    );
                    return Ok((TickOutcome::Error, decision.clone()));
                }
            }
        }
        // Step 12.
        self.count(t)?;
        if !failed.is_empty() {
            self.error(
                sink,
                format!("could not freshen {}", failed.join("; ")),
                true,
            );
            return Ok((TickOutcome::Error, decision.clone()));
        }
        Ok(self.no_switch(sink, NoSwitchReason::NoViableTarget, String::new()))
    }

    /// §11.2 step 2: an unresolved switch journal for the provider is recovered first, under
    /// `MutationGuard`, its events carrying `source` = `auto` (§9.6). `Some` with recovery's
    /// refusal while a row stays: one recovery could not decide, could not take the live
    /// locks or found its agent writing, or the mutation lock was held past its timeout.
    fn recover(&self) -> Result<Option<EngineError>, EngineError> {
        match self.engine.settle_or_refuse_as(&self.provider, "auto") {
            Ok(()) => Ok(None),
            Err(e) if e.signal().is_some() => Err(e),
            Err(
                e @ (EngineError::InterruptedSwitch(_)
                | EngineError::RecoveryBlocked { .. }
                | EngineError::RecoveryMoved { .. }
                | EngineError::Lock(LockError::Timeout(_))),
            ) => Ok(Some(e)),
            Err(e) => Err(e),
        }
    }

    /// The provider's accounts as the store holds them now, each with its decision-grade
    /// windows under the tick's models (§8.4) and its session state (§12.5), read as freshly as
    /// those readings. An account whose session state cannot be read counts as session-owned.
    fn read(&self, p: &dyn Provider, store: &Store) -> Result<Vec<Account>, EngineError> {
        store
            .accounts(&self.provider)?
            .into_iter()
            .map(|row| {
                Ok(Account {
                    windows: self.engine.decision_windows(&row, &self.cfg.models)?,
                    fetched_at: store.usage_state(&row.id)?.and_then(|s| s.fetched_at),
                    session_owned: self.engine.session_state(p, &row)?.owned(),
                    row,
                })
            })
            .collect()
    }

    /// The snapshot `decide` reads. Switchable is decided from the store alone (enabled, with
    /// an identity): the vault is read lazily, at step 10 (§9.3), where a target with no
    /// stored credential is passed over. Session ownership is as `read` found it (§11.2
    /// step 7); the switch checks it again under its locks.
    fn snapshot(&self, p: &dyn Provider, accounts: &[Account], live: &AccountId) -> Snapshot {
        Snapshot {
            now: self.engine.now_ms().div_euclid(1000),
            live: Live::Managed(live.clone()),
            accounts: accounts
                .iter()
                .map(|a| AccountSnapshot {
                    id: a.row.id.clone(),
                    position: a.row.position,
                    api_key: p.kind_traits(&a.row.kind).managed_key_axis,
                    switchable: !a.row.disabled && a.row.identity_json.is_object(),
                    quarantined: a.row.quarantine_reason.is_some(),
                    session_owned: a.session_owned,
                    windows: a.windows.clone(),
                    fetched_at: a.fetched_at,
                })
                .collect(),
        }
    }

    /// The state this tick decides with, and `unhealthy_ticks` as stored. Dry-run counts in
    /// memory. The count belongs to the account it judged (Decision 4): a live account that a
    /// switch made elsewhere since the last tick starts from 0.
    fn state(&self, store: &Store, live: &AccountId) -> Result<(AutoState, u32), EngineError> {
        let mut state = store.autoswitch_state(&self.provider)?;
        if let Some(n) = self.unhealthy.filter(|_| self.dry_run) {
            state.unhealthy_ticks = n;
        }
        let stored = state.unhealthy_ticks;
        if self.judged.as_ref().is_some_and(|judged| judged != live) {
            state.unhealthy_ticks = 0;
        }
        Ok((state, stored))
    }

    /// Writes this tick's `unhealthy_ticks` when it changed (Decision 4); dry-run keeps it.
    fn count(&mut self, t: &Ticked<'_>) -> Result<(), EngineError> {
        if self.dry_run {
            self.unhealthy = Some(t.unhealthy);
        } else if t.unhealthy != t.stored {
            t.store.set_unhealthy_ticks(&self.provider, t.unhealthy)?;
        }
        Ok(())
    }

    /// The cooldown left as `no-switch cooldown` states it, from the state as stored now.
    fn cooldown_left(&self, store: &Store) -> Result<String, EngineError> {
        let now = self.engine.now_ms().div_euclid(1000);
        Ok(store
            .autoswitch_state(&self.provider)?
            .last_switch_at
            .map_or_else(String::new, |at| {
                span(at.saturating_add(self.cfg.cooldown_s) - now)
            }))
    }

    /// Step 1's quarantine report: this tick's own releases, and, against the previous tick's
    /// map, every quarantine another process set or cleared since (Decision 10). The first
    /// tick has no previous map, so it reports only its own releases.
    fn report_quarantines(
        &mut self,
        rows: &[AccountRow],
        released: &[AccountId],
        sink: &dyn EventSink,
    ) {
        let before = self.seen.take();
        for row in rows {
            let was = before.as_ref().and_then(|m| m.get(&row.id));
            let reason = || {
                match was {
                    Some(s) if s.login_epoch != row.login_epoch => "account-replaced",
                    _ => "credentials-replaced",
                }
                .to_owned()
            };
            let cleared = released.contains(&row.id)
                || (was.is_some_and(|s| s.reason.is_some()) && row.quarantine_reason.is_none());
            let set = before.is_some()
                && row.quarantine_reason.is_some()
                && was.is_none_or(|s| s.reason.is_none());
            if cleared {
                self.emit(sink, |provider| AutoEvent::AccountUnquarantined {
                    provider,
                    number: row.position,
                    email: email_of(row),
                    reason: reason(),
                });
            } else if set {
                self.emit(sink, |provider| AutoEvent::AccountQuarantined {
                    provider,
                    number: row.position,
                    email: email_of(row),
                    reason: row.quarantine_reason.clone().unwrap_or_default(),
                });
            }
        }
        self.seen = Some(
            rows.iter()
                .map(|r| {
                    let seen = Seen {
                        reason: r.quarantine_reason.clone(),
                        login_epoch: r.login_epoch,
                    };
                    (r.id.clone(), seen)
                })
                .collect(),
        );
    }

    /// A quarantine this tick's own work found (a collection's, or step 10's freshening),
    /// reported once: the next tick's comparison already knows it.
    fn note_quarantined(&mut self, row: &AccountRow, sink: &dyn EventSink) {
        let Some(reason) = row.quarantine_reason.clone() else {
            return;
        };
        let seen = self.seen.get_or_insert_with(BTreeMap::new);
        let entry = seen.entry(row.id.clone()).or_insert(Seen {
            reason: None,
            login_epoch: row.login_epoch,
        });
        if entry.reason.is_some() {
            return;
        }
        entry.reason = Some(reason.clone());
        self.emit(sink, |provider| AutoEvent::AccountQuarantined {
            provider,
            number: row.position,
            email: email_of(row),
            reason,
        });
    }

    /// A collection's findings: the accounts it saw quarantined, and its warnings, each an
    /// `error` event that does not fail the tick (§8.3: a usage failure is never an error).
    fn report_collection(
        &mut self,
        report: &CollectReport,
        accounts: &[Account],
        sink: &dyn EventSink,
    ) {
        for id in &report.quarantined {
            if let Some(a) = accounts.iter().find(|a| &a.row.id == id) {
                self.note_quarantined(&a.row, sink);
            }
        }
        for warning in &report.warnings {
            self.error(sink, warning.clone(), true);
        }
    }

    /// Step 3's `poll` event (§11.4).
    fn poll(
        &self,
        report: &CollectReport,
        accounts: &[Account],
        live: &AccountRow,
        sink: &dyn EventSink,
    ) {
        let position = |id: &AccountId| {
            accounts
                .iter()
                .find(|a| &a.row.id == id)
                .map(|a| a.row.position)
        };
        let fetch_errors = report
            .outcomes
            .iter()
            .filter_map(|(id, collected)| {
                let kind = match collected {
                    Collected::Failed { kind } => kind.clone(),
                    Collected::OverBudget { .. } => "over-budget".to_owned(),
                    _ => return None,
                };
                Some((position(id)?, kind))
            })
            .collect();
        let headroom_pct = accounts
            .iter()
            .map(|a| {
                let h = a
                    .windows
                    .as_deref()
                    .and_then(|w| headroom(w, &self.cfg.models));
                (a.row.position, h)
            })
            .collect();
        let windows_pct = accounts
            .iter()
            .filter_map(|a| {
                let windows = a.windows.as_ref()?;
                let pct = windows.iter().map(|w| (w.key.clone(), w.pct)).collect();
                Some((a.row.position, pct))
            })
            .collect();
        self.emit(sink, |provider| AutoEvent::Poll {
            provider,
            active: Some((live.position, email_of(live))),
            headroom_pct,
            threshold: self.cfg.threshold,
            fetch_errors,
            windows_pct,
        });
    }

    /// Step 3's settings checks, once per engine and again when the setting changes: a
    /// consume-first strategy on a provider without a long window runs `best` (§4.5), and
    /// each configured model name should be a scoped window some account's reading reports.
    /// The model check waits for a tick on which some account has a reading.
    fn check_settings(
        &mut self,
        p: &dyn Provider,
        store: &Store,
        accounts: &[Account],
        sink: &dyn EventSink,
    ) -> Result<(), EngineError> {
        if self.checked_strategy != Some(self.cfg.strategy) {
            if self.cfg.effective_strategy() != self.cfg.strategy {
                let message = format!(
                    "{} has no long usage window to rank by, so consume-first runs best",
                    p.display_name()
                );
                self.emit(sink, |provider| AutoEvent::ConfigWarning {
                    provider,
                    message,
                });
            }
            self.checked_strategy = Some(self.cfg.strategy);
        }
        if self.checked_models.as_ref() == Some(&self.cfg.models) {
            return Ok(());
        }
        let names: Vec<&String> = self
            .cfg
            .models
            .iter()
            .filter(|m| !m.eq_ignore_ascii_case("all"))
            .collect();
        if !names.is_empty() {
            let mut read_any = false;
            let mut scoped = BTreeSet::new();
            for a in accounts {
                if let Some(windows) = store.usage_state(&a.row.id)?.and_then(|s| s.last_good) {
                    read_any = true;
                    scoped.extend(
                        windows
                            .iter()
                            .filter(|w| w.kind == WindowKind::Scoped)
                            .map(|w| w.label.to_lowercase()),
                    );
                }
            }
            if !read_any {
                return Ok(());
            }
            for name in names {
                if !scoped.contains(&name.to_lowercase()) {
                    let message = format!(
                        "autoswitch.models names {name:?}, but no account's usage reports a window for that model"
                    );
                    self.emit(sink, |provider| AutoEvent::ConfigWarning {
                        provider,
                        message,
                    });
                }
            }
        }
        self.checked_models = Some(self.cfg.models.clone());
        Ok(())
    }

    fn emit(&self, sink: &dyn EventSink, event: impl FnOnce(ProviderId) -> AutoEvent) {
        sink.emit(&event(self.provider.clone()));
    }

    fn error(&self, sink: &dyn EventSink, message: String, transient: bool) {
        self.emit(sink, |provider| AutoEvent::Error {
            provider,
            message,
            transient,
        });
    }

    /// A `no-switch` the engine reports itself (§11.2 steps 2, 11 and 12), announced.
    fn no_switch(
        &self,
        sink: &dyn EventSink,
        reason: NoSwitchReason,
        detail: String,
    ) -> (TickOutcome, Decision) {
        let decision = Decision::NoSwitch {
            reason,
            outcome: reason.outcome(),
            detail,
            earliest_reset: None,
        };
        self.announce(sink, &decision);
        (TickOutcome::of(&decision), decision)
    }

    /// `no-switch`, then `all-exhausted` with its earliest recovery when that is the reason.
    fn announce(&self, sink: &dyn EventSink, decision: &Decision) {
        let Decision::NoSwitch {
            reason,
            detail,
            earliest_reset,
            ..
        } = decision
        else {
            return;
        };
        self.emit(sink, |provider| AutoEvent::NoSwitch {
            provider,
            reason: reason.as_str().to_owned(),
            detail: detail.clone(),
        });
        if *reason == NoSwitchReason::AllExhausted {
            self.emit(sink, |provider| AutoEvent::AllExhausted {
                provider,
                earliest_reset_at: *earliest_reset,
            });
        }
    }
}

/// What a tick has read by the time it decided.
struct Ticked<'a> {
    store: &'a Store,
    live: &'a AccountRow,
    snap: Snapshot,
    /// `unhealthy_ticks` as stored, and as this tick decided it.
    stored: u32,
    unhealthy: u32,
}

/// Consume-first's re-check (§11.2 step 8): the current account and every OAuth candidate.
fn recheck_ids(snap: &Snapshot, live: &AccountId) -> Vec<AccountId> {
    let candidates = snap.accounts.iter().filter(|a| {
        &a.id != live && a.switchable && !a.quarantined && !a.session_owned && !a.api_key
    });
    std::iter::once(live.clone())
        .chain(candidates.map(|a| a.id.clone()))
        .collect()
}
