use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

use tagteam_core::pace::pace;
use tagteam_core::trust::decision_grade;
use tagteam_core::usage::{earliest_relevant_reset, is_relevant};
use tagteam_core::{AccountId, Pace, PollBudget, ProviderId, Sample, TrustInputs, Window};
use tagteam_provider::{Identity, KindTraits, Provider, Read, RunShell};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{
    AccountRow, LiveIdentityCacheRow, Store, UsageStateRow, backoff_is_skewed, plan_is_skewed,
};

/// §8.7: pace and projections read the samples of the 48 h before a reading.
const PACE_LOOKBACK_S: i64 = 48 * 3600;

/// The longest `statusline` waits on the store's write lock to cache the live identity: the
/// cache is a convenience, and the line is never held up for it (§13.5).
const CACHE_WRITE_WAIT: Duration = Duration::from_millis(5);

/// `usageError` for an account that has neither a reading nor a failure yet (§13.2).
pub const NO_DATA: &str = "no-data";

/// §13.2's `usageStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageStatus {
    Ok,
    TokenExpired,
    ApiKey,
    KeychainUnavailable,
    ReloginRequired,
    ForeignCredential,
    NoCredentials,
    Unavailable,
    Unsupported,
}

impl UsageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            UsageStatus::Ok => "ok",
            UsageStatus::TokenExpired => "token_expired",
            UsageStatus::ApiKey => "api_key",
            UsageStatus::KeychainUnavailable => "keychain_unavailable",
            UsageStatus::ReloginRequired => "relogin_required",
            UsageStatus::ForeignCredential => "foreign_credential",
            UsageStatus::NoCredentials => "no_credentials",
            UsageStatus::Unavailable => "unavailable",
            UsageStatus::Unsupported => "unsupported",
        }
    }
}

/// An account's usage as the views show it (§8.4, §13.2).
#[derive(Debug, Clone, PartialEq)]
pub struct UsageView {
    pub status: UsageStatus,
    /// The last good reading, each window with its pace (§8.7); in `statusline`'s view, which
    /// never shows pace, every pace is `Pace::default()`.
    pub windows: Option<Vec<(Window, Pace)>>,
    /// Whether the reading may drive a decision and be shown as `usage` (§8.4).
    pub decision_grade: bool,
    pub fetched_at: Option<i64>,
    pub age_s: Option<i64>,
    /// Why the status is not `ok`: the `last_error` of the failures being retried (§13.2), and
    /// for `Unavailable` `no-data` when there is none. `None` when the status is `ok`, or no
    /// failure is on record (a quarantined account, an API key).
    pub error: Option<String>,
    /// When the next fetch may happen, `max(backoff_until, next_poll_at)`, while that is still
    /// ahead and the status is one that is retried: not `ok`, and not `ReloginRequired`,
    /// `ApiKey` or `Unsupported`, which are never fetched.
    pub retry_at: Option<i64>,
}

impl UsageView {
    /// No reading to show: `status`, and for `Unavailable` why (`no-data` unless `error`).
    fn unread(status: UsageStatus, error: Option<String>) -> Self {
        let unavailable = status == UsageStatus::Unavailable;
        UsageView {
            status,
            windows: None,
            decision_grade: false,
            fetched_at: None,
            age_s: None,
            error: unavailable.then(|| error.unwrap_or_else(|| NO_DATA.to_owned())),
            retry_at: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AccountView {
    pub row: AccountRow,
    /// The live identity wins over the store's active account.
    pub active: bool,
    /// The row's credential kind, as its provider describes it (§4.5).
    pub kind: KindTraits,
    /// Its usage, from the store alone (§13.2).
    pub usage: UsageView,
    /// Session-owned (§12.5): a live launch reservation, or a session record that is live or
    /// cannot be read (§12.6). `list` marks it `▶`, and its row says `inSession` (§13.1,
    /// §13.2). Computed for `list`, `status` and the account commands' results; the
    /// statusline's view leaves it `false`, so the status bar never reads a profile directory
    /// (§13.5).
    pub in_session: bool,
}

/// The kind traits of a row whose provider this build does not register: nothing special.
const UNREGISTERED: KindTraits = KindTraits {
    refreshable: false,
    managed_key_axis: false,
    default_email_prefix: None,
    display: None,
};

#[derive(Debug, Clone)]
pub struct ProviderAccounts {
    pub provider: ProviderId,
    pub active_position: Option<u32>,
    pub accounts: Vec<AccountView>,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum StatusView {
    NoLogin,
    Unmanaged { email: String },
    Managed { account: AccountView, total: usize },
}

/// The run shell's own account (§12.8, §13.2's `session`), as its marker names it.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum ShellAccount {
    /// Not in a run shell.
    NotInShell,
    /// The account the marker names, as the store holds it.
    Managed(AccountRow),
    /// The marker names an account tagteam does not manage: the store lacks it, holds it under
    /// another provider, or does not exist.
    Unmanaged,
}

/// Decision 10's table; the first row that matches wins. `state` is the account's
/// `usage_state` row, if any. Its `last_error` counts only while failures are being retried:
/// a success resets them. A reading is a `fetched_at`: Task 8 stores a reading with no windows
/// as a null `last_good`.
fn usage_status(
    supported: bool,
    kind: &KindTraits,
    row: &AccountRow,
    state: Option<&UsageStateRow>,
) -> UsageStatus {
    if !supported {
        return UsageStatus::Unsupported;
    }
    if kind.managed_key_axis {
        return UsageStatus::ApiKey;
    }
    if row.quarantine_reason.is_some() {
        return UsageStatus::ReloginRequired;
    }
    let failing = state
        .filter(|s| s.consecutive_failures > 0)
        .and_then(|s| s.last_error.as_deref());
    match failing {
        Some("foreign-credential") => UsageStatus::ForeignCredential,
        Some("keychain-unavailable") => UsageStatus::KeychainUnavailable,
        Some("no-access-token" | "vault-absent") => UsageStatus::NoCredentials,
        Some("token-expired") => UsageStatus::TokenExpired,
        Some(_) => UsageStatus::Unavailable,
        None if state.is_some_and(|s| s.fetched_at.is_some()) => UsageStatus::Ok,
        None => UsageStatus::Unavailable,
    }
}

/// §8.7's pace of `w` as of `fetched_at`, from the samples of its window that the reading may
/// see: the 48 h up to it, and none taken later. `samples` may hold more; the one filter lives
/// here, so the list and `history` pace alike.
fn pace_from(w: &Window, fetched_at: i64, samples: &[Sample]) -> Pace {
    let seen: Vec<Sample> = samples
        .iter()
        .copied()
        .filter(|s| (fetched_at - PACE_LOOKBACK_S..=fetched_at).contains(&s.fetched_at))
        .collect();
    pace(w, fetched_at, &seen)
}

/// The reading's windows, each with §8.7's pace from its own samples of the 48 h before the
/// reading: one query per window, along the samples' key (account, window, time).
fn paced(
    store: &Store,
    id: &AccountId,
    windows: &[Window],
    fetched_at: i64,
) -> Result<Vec<(Window, Pace)>, EngineError> {
    windows
        .iter()
        .map(|w| {
            let samples: Vec<Sample> = store
                .usage_samples(id, Some(&w.key), fetched_at - PACE_LOOKBACK_S)?
                .into_iter()
                .map(|(_, s)| s)
                .collect();
            Ok((w.clone(), pace_from(w, fetched_at, &samples)))
        })
        .collect()
}

/// One window of `tagteam history` (§13.4): its definition, its samples since the requested
/// time, and §8.7's pace as of its reading. A window of the last good reading is as read then;
/// one only the samples name is described by the provider (`Provider::describe_window`) and
/// read as of its latest sample, whose `pct` and reset it carries.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryWindow {
    pub window: Window,
    pub samples: Vec<Sample>,
    pub pace: Pace,
}

#[derive(Debug, Clone)]
pub struct HistoryView {
    pub account: AccountView,
    pub windows: Vec<HistoryWindow>,
    /// A window filter was given and the account has windows, but none is the one asked for.
    pub unmatched_window: bool,
}

/// What `statusline` shows (§13.5).
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum StatuslineView {
    NoLogin,
    Unmanaged { email: String },
    Managed { account: AccountView },
}

/// The live login as `statusline` needs it: its identity key and its label (the email when it
/// has one).
struct LiveLogin {
    key: String,
    label: String,
    account_uuid: Option<String>,
}

/// The provider's live login as the account views read it.
struct Liveness {
    identity: Read<Identity>,
    key: Read<String>,
    /// The store's active account, read only while the live identity is unreadable.
    stored_active: Option<AccountId>,
}

impl Liveness {
    fn is_active(&self, row: &AccountRow) -> bool {
        match &self.key {
            Read::Present(k) => &row.identity_key == k,
            Read::Absent => false,
            Read::Unreadable(_) => self.stored_active.as_ref() == Some(&row.id),
        }
    }
}

impl LiveLogin {
    fn of(p: &dyn Provider, i: &Identity) -> Self {
        LiveLogin {
            key: p.identity_key(i).as_str().to_owned(),
            label: i.email.clone().unwrap_or_else(|| i.label.clone()),
            account_uuid: i.account_uuid.clone(),
        }
    }
}

impl Engine {
    /// §13.2's usage for one account: its status (Decision 10), its last good reading with
    /// pace, and whether that reading is decision-grade (§8.4). Reads the store only.
    ///
    /// `with_pace` is whether each window carries §8.7's pace, which costs a query per window
    /// of samples; a view that never shows it asks for none (`Pace::default()`).
    fn usage_view(
        &self,
        store: Option<&Store>,
        row: &AccountRow,
        kind: &KindTraits,
        supported: bool,
        budget: &PollBudget,
        with_pace: bool,
    ) -> Result<UsageView, EngineError> {
        let state = match store {
            Some(s) if supported => s.usage_state(&row.id)?,
            _ => None,
        };
        let status = usage_status(supported, kind, row, state.as_ref());
        let (Some(store), Some(state)) = (store, state) else {
            return Ok(UsageView::unread(status, None));
        };
        let now_s = self.now_ms().div_euclid(1000);
        let windows = match state.fetched_at {
            Some(at) => {
                let read = state.last_good.as_deref().unwrap_or_default();
                Some(if with_pace {
                    paced(store, &row.id, read, at)?
                } else {
                    read.iter().map(|w| (w.clone(), Pace::default())).collect()
                })
            }
            None => None,
        };
        let trusted = windows.is_some()
            && self.is_decision_grade(store, row, &state, &self.settings().models, budget)?;
        let failing = state
            .last_error
            .clone()
            .filter(|_| state.consecutive_failures > 0);
        let error = match status {
            UsageStatus::Ok => None,
            UsageStatus::Unavailable => Some(failing.unwrap_or_else(|| NO_DATA.to_owned())),
            _ => failing,
        };
        let retried = !matches!(
            status,
            UsageStatus::Ok
                | UsageStatus::ReloginRequired
                | UsageStatus::ApiKey
                | UsageStatus::Unsupported
        );
        Ok(UsageView {
            status,
            windows,
            decision_grade: trusted,
            fetched_at: state.fetched_at,
            age_s: state.fetched_at.map(|at| (now_s - at).max(0)),
            error,
            // The times `reserve_usage` ignores as clock skew (§8.4) are no retry time either.
            retry_at: state
                .backoff_until
                .filter(|&at| !backoff_is_skewed(at, now_s))
                .max(
                    state
                        .next_poll_at
                        .filter(|&at| !plan_is_skewed(at, now_s, budget)),
                )
                .filter(|&at| retried && at > now_s),
        })
    }

    /// §8.4 for `row`'s reading in `state`: whether it may drive a decision. Relevance, which
    /// sets the post-429 rule's earliest reset, follows `models` (§8.2), and a planned poll
    /// further out than `budget` allows is clock skew, not a plan in force. Reads the store
    /// only.
    fn is_decision_grade(
        &self,
        store: &Store,
        row: &AccountRow,
        state: &UsageStateRow,
        models: &[String],
        budget: &PollBudget,
    ) -> Result<bool, EngineError> {
        let now_ms = self.now_ms();
        let now_s = now_ms.div_euclid(1000);
        // §8.4 extends trust only while failures are being retried, and a quarantined account
        // is never retried: it keeps the five-minute rule alone.
        let retried = row.quarantine_reason.is_none();
        Ok(decision_grade(&TrustInputs {
            now_s,
            fetched_at: state.fetched_at,
            consecutive_failures: if retried {
                state.consecutive_failures
            } else {
                0
            },
            // A plan further out than any legal one is clock skew (§8.4), not a plan.
            plan_in_force: retried
                && state
                    .next_poll_at
                    .is_some_and(|at| at > now_s && !plan_is_skewed(at, now_s, budget)),
            live_lease: store.usage_lease_live(&row.id, now_ms)?,
            last_429_at: state.last_429_at,
            earliest_relevant_reset: state
                .last_good
                .as_deref()
                .and_then(|w| earliest_relevant_reset(w, models)),
        }))
    }

    /// The account's last good windows if they are decision-grade (§8.4) under `models`'
    /// relevance; `None` otherwise. Reads the store only.
    pub fn decision_windows(
        &self,
        row: &AccountRow,
        models: &[String],
    ) -> Result<Option<Vec<Window>>, EngineError> {
        let Some(store) = self.existing_store()? else {
            return Ok(None);
        };
        let Some(state) = store.usage_state(&row.id)? else {
            return Ok(None);
        };
        let budget = self
            .registry
            .get(&row.provider)
            .map_or(PollBudget::STANDARD, |p| p.poll_budget());
        if state.fetched_at.is_none()
            || !self.is_decision_grade(&store, row, &state, models, &budget)?
        {
            return Ok(None);
        }
        Ok(Some(state.last_good.unwrap_or_default()))
    }

    fn view(&self, provider: &ProviderId) -> Result<(ProviderAccounts, Read<String>), EngineError> {
        let p = self.provider(provider)?;
        let store = self.existing_store()?;
        let rows = match &store {
            Some(s) => s.accounts(provider)?,
            None => vec![],
        };
        let live = self.liveness(p.as_ref(), store.as_deref(), provider)?;
        let supported = p.capabilities().usage;
        let accounts = rows
            .into_iter()
            .map(|row| {
                let active = live.is_active(&row);
                self.account_view_of(p.as_ref(), store.as_deref(), row, active, supported, true)
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        let active_position = accounts.iter().find(|v| v.active).map(|v| v.row.position);
        let live_label = live.identity.map(|i| i.email.unwrap_or(i.label));
        Ok((
            ProviderAccounts {
                provider: provider.clone(),
                active_position,
                accounts,
            },
            live_label,
        ))
    }

    /// The provider's live login, and what decides which account is active: the live identity
    /// wins, and the store's active account stands in while the live identity is unreadable.
    fn liveness(
        &self,
        p: &dyn Provider,
        store: Option<&Store>,
        provider: &ProviderId,
    ) -> Result<Liveness, EngineError> {
        let identity = p.live_identity(&self.env);
        let key = identity
            .as_ref()
            .map(|i| p.identity_key(i).as_str().to_owned());
        let stored_active = match (&key, store) {
            (Read::Unreadable(_), Some(s)) => s.active(provider)?,
            _ => None,
        };
        Ok(Liveness {
            identity,
            key,
            stored_active,
        })
    }

    /// One account as the views show it, its usage read from `store` (with pace, or without:
    /// see `usage_view`).
    fn account_view_of(
        &self,
        p: &dyn Provider,
        store: Option<&Store>,
        row: AccountRow,
        active: bool,
        supported: bool,
        with_pace: bool,
    ) -> Result<AccountView, EngineError> {
        let kind = p.kind_traits(&row.kind);
        let usage = self.usage_view(store, &row, &kind, supported, &p.poll_budget(), with_pace)?;
        let in_session = self.in_session(p, &row);
        Ok(AccountView {
            kind,
            row,
            active,
            usage,
            in_session,
        })
    }

    /// A row as the views show it, for a caller that already knows whether it is active. Its
    /// usage comes from the store; a store that cannot be read leaves it unread, logged, since
    /// the callers report a change that has already happened. Whether it is session-owned is
    /// computed, as for `list`.
    pub fn account_view(&self, row: AccountRow, active: bool) -> AccountView {
        // A provider this build does not register has no profile it could run.
        let in_session = self
            .registry
            .get(&row.provider)
            .is_some_and(|p| self.in_session(p.as_ref(), &row));
        self.account_view_with(row, active, true, in_session)
    }

    /// `account_view`, with or without pace on its windows (see `usage_view`), and with
    /// `in_session` as the caller knows it. It is never computed here: `statusline` passes
    /// `false`, which keeps the status bar away from profile directories (§13.5, Decision 17).
    fn account_view_with(
        &self,
        row: AccountRow,
        active: bool,
        with_pace: bool,
        in_session: bool,
    ) -> AccountView {
        let provider = self.registry.get(&row.provider);
        let kind = provider
            .as_ref()
            .map_or(UNREGISTERED, |p| p.kind_traits(&row.kind));
        let budget = provider
            .as_ref()
            .map_or(PollBudget::STANDARD, |p| p.poll_budget());
        let supported = provider.is_some_and(|p| p.capabilities().usage);
        let usage = self
            .existing_store()
            .and_then(|s| self.usage_view(s.as_deref(), &row, &kind, supported, &budget, with_pace))
            .unwrap_or_else(|e| {
                tracing::warn!(
                    position = row.position,
                    id = %row.id,
                    kind = e.kind(),
                    "could not read the account's usage"
                );
                UsageView::unread(
                    usage_status(supported, &kind, &row, None),
                    Some(e.kind().to_owned()),
                )
            });
        AccountView {
            row,
            active,
            kind,
            usage,
            in_session,
        }
    }

    /// Whether `row` is session-owned, as `list`, `status` and the account commands mark it
    /// (§12.5, §13.1); `statusline` never asks (Decision 17). It is computed on each call
    /// (Decision 8): `session_state` answers `NoProfile` after one look at the
    /// profile directory, which most accounts lack. A state that cannot be determined counts as
    /// owned, as everywhere (§12.6), and is logged.
    fn in_session(&self, p: &dyn Provider, row: &AccountRow) -> bool {
        if !p.capabilities().sessions {
            return false;
        }
        match self.session_state(p, row) {
            Ok(state) => state.owned(),
            Err(e) => {
                tracing::warn!(
                    position = row.position,
                    id = %row.id,
                    kind = e.kind(),
                    "could not tell whether the account is in a session; marking it in session"
                );
                true
            }
        }
    }

    /// The run shell's own account (§12.8), as its marker names it: by id, from the store, with
    /// no live-identity read and no `.claude.json` parse (§13.5). `NotInShell` outside one. A
    /// marker naming an account the store does not hold, holds under another provider, or with
    /// no store at all, is `Unmanaged`. An unreadable marker is the refusal every command but
    /// `statusline` gives (§12.8).
    pub fn shell_account(&self) -> Result<ShellAccount, EngineError> {
        match self.run_shell() {
            RunShell::Outside => Ok(ShellAccount::NotInShell),
            RunShell::Unreadable { marker, detail } => Err(EngineError::RunShellUnreadable {
                marker: marker.clone(),
                detail: detail.clone(),
            }),
            RunShell::Inside { marker, .. } => {
                let Some(store) = self.existing_store()? else {
                    return Ok(ShellAccount::Unmanaged);
                };
                Ok(match store.account(&marker.account_id)? {
                    Some(row) if row.provider == marker.provider => ShellAccount::Managed(row),
                    Some(_) | None => ShellAccount::Unmanaged,
                })
            }
        }
    }

    /// Every provider that has accounts, plus the default provider; or just `provider`.
    pub fn accounts(
        &self,
        provider: Option<&ProviderId>,
    ) -> Result<Vec<ProviderAccounts>, EngineError> {
        let ids: Vec<ProviderId> = match provider {
            Some(p) => vec![p.clone()],
            None => {
                let mut ids = vec![self.default_provider.clone()];
                if let Some(s) = self.existing_store()? {
                    for row in s.all_accounts()? {
                        if !ids.contains(&row.provider)
                            && self.registry.get(&row.provider).is_some()
                        {
                            ids.push(row.provider);
                        }
                    }
                }
                ids
            }
        };
        ids.iter().map(|id| self.view(id).map(|(v, _)| v)).collect()
    }

    pub fn status(&self, provider: &ProviderId) -> Result<StatusView, EngineError> {
        let (list, live) = self.view(provider)?;
        let total = list.accounts.len();
        if let Some(active) = list.accounts.into_iter().find(|v| v.active) {
            return Ok(StatusView::Managed {
                account: active,
                total,
            });
        }
        match live {
            Read::Present(email) => Ok(StatusView::Unmanaged { email }),
            Read::Absent => Ok(StatusView::NoLogin),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }

    /// §13.4, reading only: `account`'s windows, each with its samples fetched at or after
    /// `since_s` and its pace as of its reading. The windows are those of the last good
    /// reading, then, by key, those that only samples since `since_s` name (a window the latest
    /// reading lacks, or every window after an empty one), as the provider describes them;
    /// a key it does not recognise is skipped. Of these, the relevant ones (§8.2), or those
    /// whose key or label is `window`, ignoring case. An account never read has no windows.
    pub fn history(
        &self,
        account: &AccountId,
        window: Option<&str>,
        since_s: i64,
    ) -> Result<HistoryView, EngineError> {
        let missing = || EngineError::NoSuchAccount(account.to_string());
        let store = self.existing_store()?.ok_or_else(missing)?;
        let row = store.account(account)?.ok_or_else(missing)?;
        let p = self.provider(&row.provider)?;
        let live = self.liveness(p.as_ref(), Some(&store), &row.provider)?;
        let view = self.account_view_of(
            p.as_ref(),
            Some(&store),
            row.clone(),
            live.is_active(&row),
            p.capabilities().usage,
            true,
        )?;
        // Each window with the time its `pct` is as of, and its pace when the view has it: the
        // last good reading's own windows are the view's, as read then.
        let mut read: Vec<(Window, i64, Option<Pace>)> =
            match (&view.usage.windows, view.usage.fetched_at) {
                (Some(windows), Some(at)) => windows
                    .iter()
                    .map(|(w, pace)| (w.clone(), at, Some(*pace)))
                    .collect(),
                _ => Vec::new(),
            };
        // Samples ascend by time, so the last one kept per key is that window's latest.
        let mut latest: BTreeMap<String, Sample> = BTreeMap::new();
        for (key, sample) in store.usage_samples(&row.id, None, since_s)? {
            latest.insert(key, sample);
        }
        for (key, s) in latest {
            if read.iter().any(|(w, ..)| w.key == key) {
                continue;
            }
            if let Some(w) = p.describe_window(&key) {
                let w = Window {
                    pct: s.pct,
                    resets_at: s.resets_at,
                    ..w
                };
                read.push((w, s.fetched_at, None));
            }
        }
        let had_windows = !read.is_empty();
        let models = &self.settings().models;
        let windows = read
            .into_iter()
            .filter(|(w, ..)| match window {
                Some(name) => {
                    w.key.eq_ignore_ascii_case(name) || w.label.eq_ignore_ascii_case(name)
                }
                None => is_relevant(w, models),
            })
            .map(|(w, fetched_at, known)| {
                let (pace, samples) = match known {
                    // The view's pace is as of the same reading: only the samples to show.
                    Some(pace) => {
                        let shown = store.usage_samples(&row.id, Some(&w.key), since_s)?;
                        (pace, shown.into_iter().map(|(_, s)| s).collect())
                    }
                    // One query reaches back to the earlier of `since_s` and pace's lookback.
                    None => {
                        let all: Vec<Sample> = store
                            .usage_samples(
                                &row.id,
                                Some(&w.key),
                                since_s.min(fetched_at - PACE_LOOKBACK_S),
                            )?
                            .into_iter()
                            .map(|(_, s)| s)
                            .collect();
                        let pace = pace_from(&w, fetched_at, &all);
                        (
                            pace,
                            all.into_iter()
                                .filter(|s| s.fetched_at >= since_s)
                                .collect(),
                        )
                    }
                };
                Ok(HistoryWindow {
                    window: w,
                    samples,
                    pace,
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        Ok(HistoryView {
            account: view,
            unmatched_window: window.is_some() && had_windows && windows.is_empty(),
            windows,
        })
    }

    /// §13.5: the line's account and, when tagteam manages it, its usage. No network, no
    /// Keychain, and the store is never created.
    /// - In a run shell for `provider` (§12.8), the account the marker names, by id
    ///   (`shell_account`): no `.claude.json` is parsed, the profile's or the default home's,
    ///   and one tagteam does not manage shows nothing.
    /// - Under an unreadable marker, nothing: the outer home is unknown (§12.8).
    /// - Otherwise the live login, from `live_identity_cache` while
    ///   `Provider::live_identity_source`'s mtime and size are unchanged, re-parsed only when
    ///   they change. A missing, unreadable or garbled source is `NoLogin`, never an error.
    pub fn statusline(&self, provider: &ProviderId) -> Result<StatuslineView, EngineError> {
        let p = self.provider(provider)?;
        match self.run_shell() {
            RunShell::Unreadable { .. } => return Ok(StatuslineView::NoLogin),
            RunShell::Inside { marker, .. } if &marker.provider == provider => {
                return Ok(match self.shell_account()? {
                    // The session's login, not the default home's: not `active`. The line
                    // shows no pace, so none is computed, and it never asks whether the
                    // account is in a session (Decision 17): the status bar stays away from
                    // profile directories.
                    ShellAccount::Managed(row) => StatuslineView::Managed {
                        account: self.account_view_with(row, false, false, false),
                    },
                    ShellAccount::Unmanaged | ShellAccount::NotInShell => StatuslineView::NoLogin,
                });
            }
            RunShell::Inside { .. } | RunShell::Outside => {}
        }
        let store = self.existing_store()?;
        let Some(login) = self.live_login(p.as_ref(), store.as_deref())? else {
            return Ok(StatuslineView::NoLogin);
        };
        let row = match &store {
            Some(s) => s.find_by_identity_key(provider, &login.key)?,
            None => None,
        };
        Ok(match row {
            // The line shows no pace, so none is computed.
            Some(row) => StatuslineView::Managed {
                // Nor does it ask whether the account is in a session: the status bar stays away
                // from profile directories (§13.5, Decision 17).
                account: self.account_view_with(row, true, false, false),
            },
            None => StatuslineView::Unmanaged { email: login.label },
        })
    }

    /// The live login, through `live_identity_cache` (§13.5). Without a store nothing is
    /// cached and the source is parsed every time.
    fn live_login(
        &self,
        p: &dyn Provider,
        store: Option<&Store>,
    ) -> Result<Option<LiveLogin>, EngineError> {
        let parse = || match p.live_identity(&self.env) {
            Read::Present(i) => Some(Some(LiveLogin::of(p, &i))),
            Read::Absent => Some(None),
            Read::Unreadable(_) => None,
        };
        let (Some(path), Some(store)) = (p.live_identity_source(&self.env), store) else {
            return Ok(parse().flatten());
        };
        // Stat before parsing: a rewrite in between leaves the old stamp on the new identity,
        // which the next run's stat sees and parses again; never a new stamp on an old one.
        let Ok(meta) = std::fs::metadata(&path) else {
            return Ok(None);
        };
        let stamp = LiveIdentityCacheRow {
            provider: p.id(),
            path: path.to_string_lossy().into_owned(),
            mtime_ns: meta
                .mtime()
                .saturating_mul(1_000_000_000)
                .saturating_add(meta.mtime_nsec()),
            size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
            identity_key: None,
            label: None,
            account_uuid: None,
        };
        if let Some(c) = store.live_identity_cache(&stamp.provider)? {
            if c.path == stamp.path && c.mtime_ns == stamp.mtime_ns && c.size == stamp.size {
                return Ok(c.identity_key.map(|key| LiveLogin {
                    key,
                    label: c.label.unwrap_or_default(),
                    account_uuid: c.account_uuid,
                }));
            }
        }
        // An unreadable or garbled file is not cached: the next run parses it again.
        let Some(login) = parse() else {
            return Ok(None);
        };
        let row = LiveIdentityCacheRow {
            identity_key: login.as_ref().map(|l| l.key.clone()),
            label: login.as_ref().map(|l| l.label.clone()),
            account_uuid: login.as_ref().and_then(|l| l.account_uuid.clone()),
            ..stamp
        };
        // Only a cache: a failed write costs the next run a parse, never this one its line.
        if let Err(e) = store.put_live_identity_cache_within(&row, CACHE_WRITE_WAIT) {
            tracing::debug!(error = %e, "the live identity cache was not written");
        }
        Ok(login)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tagteam_core::{CLAUDE_CODE, WindowKind};

    use super::*;

    const OAUTH: KindTraits = KindTraits {
        refreshable: true,
        managed_key_axis: false,
        default_email_prefix: None,
        display: None,
    };

    const API_KEY: KindTraits = KindTraits {
        refreshable: false,
        managed_key_axis: true,
        default_email_prefix: Some("api-key"),
        display: Some("api key"),
    };

    fn row(quarantined: bool) -> AccountRow {
        AccountRow {
            id: AccountId::from_string("0192"),
            provider: ProviderId::new(CLAUDE_CODE),
            position: 1,
            identity_key: "a@x.co\n".into(),
            label: "a@x.co".into(),
            email: Some("a@x.co".into()),
            org_uuid: String::new(),
            org_name: None,
            account_uuid: None,
            kind: "oauth".into(),
            alias: None,
            disabled: false,
            identity_json: json!({}),
            login_expires_at: None,
            login_epoch: 0,
            replacing_fp: None,
            quarantine_reason: quarantined.then(|| "invalid_grant".into()),
            quarantine_fp: None,
            quarantine_at: None,
            added_at: 1,
        }
    }

    /// A `usage_state` row with `failures` and `last_error`, and a reading when `read`.
    fn state(failures: u32, last_error: Option<&str>, read: bool) -> UsageStateRow {
        UsageStateRow {
            account_id: AccountId::from_string("0192"),
            last_good: read.then(|| {
                vec![Window {
                    key: "5h".into(),
                    label: "5h".into(),
                    kind: WindowKind::Short,
                    pct: 9.0,
                    resets_at: None,
                    period_s: None,
                    detail: None,
                }]
            }),
            fetched_at: read.then_some(1),
            last_attempt_at: None,
            consecutive_failures: failures,
            last_error: last_error.map(str::to_owned),
            backoff_until: None,
            next_poll_at: None,
            poll_interval_s: None,
            last_429_at: None,
            rejected_fp: None,
        }
    }

    #[test]
    fn usage_status_follows_the_table_row_by_row() {
        use UsageStatus::*;
        let status =
            |supported: bool, kind: KindTraits, quarantined: bool, state: Option<UsageStateRow>| {
                usage_status(supported, &kind, &row(quarantined), state.as_ref())
            };
        let failing = |error: &str| Some(state(1, Some(error), true));
        // The first row that matches wins.
        assert_eq!(
            status(false, API_KEY, true, failing("http-429")),
            Unsupported
        );
        assert_eq!(status(true, API_KEY, true, failing("http-429")), ApiKey);
        assert_eq!(
            status(true, OAUTH, true, failing("foreign-credential")),
            ReloginRequired
        );
        for (error, want) in [
            ("foreign-credential", ForeignCredential),
            ("keychain-unavailable", KeychainUnavailable),
            ("no-access-token", NoCredentials),
            ("vault-absent", NoCredentials),
            ("token-expired", TokenExpired),
            ("http-429", Unavailable),
            ("refresh-failed", Unavailable),
            ("live-replaced", Unavailable),
            ("profile-drifted", Unavailable),
        ] {
            assert_eq!(status(true, OAUTH, false, failing(error)), want, "{error}");
        }
        // A last_error left from before a success no longer counts.
        let succeeded = Some(state(0, Some("http-429"), true));
        assert_eq!(status(true, OAUTH, false, succeeded), Ok);
        assert_eq!(status(true, OAUTH, false, Some(state(0, None, true))), Ok);
        // A reading of no windows is still a reading.
        let empty = UsageStateRow {
            last_good: None,
            ..state(0, None, true)
        };
        assert_eq!(status(true, OAUTH, false, Some(empty)), Ok);
        // Never read, but refused by the hourly budget: unavailable for that reason (§8.6).
        assert_eq!(
            status(
                true,
                OAUTH,
                false,
                Some(state(1, Some("over-budget"), false))
            ),
            Unavailable
        );
        // Never read and never failed: no data yet.
        assert_eq!(
            status(true, OAUTH, false, Some(state(0, None, false))),
            Unavailable
        );
        assert_eq!(status(true, OAUTH, false, None), Unavailable);
    }

    #[test]
    fn usage_status_strings_are_pinned() {
        use UsageStatus::*;
        let all = [
            (Ok, "ok"),
            (TokenExpired, "token_expired"),
            (ApiKey, "api_key"),
            (KeychainUnavailable, "keychain_unavailable"),
            (ReloginRequired, "relogin_required"),
            (ForeignCredential, "foreign_credential"),
            (NoCredentials, "no_credentials"),
            (Unavailable, "unavailable"),
            (Unsupported, "unsupported"),
        ];
        for (status, text) in all {
            assert_eq!(status.as_str(), text);
        }
    }

    #[test]
    fn a_view_without_a_reading_says_why_only_when_unavailable() {
        let none = UsageView::unread(UsageStatus::Unavailable, None);
        assert_eq!(none.error.as_deref(), Some(NO_DATA));
        let store = UsageView::unread(UsageStatus::Unavailable, Some("store".into()));
        assert_eq!(store.error.as_deref(), Some("store"));
        let key = UsageView::unread(UsageStatus::ApiKey, Some("store".into()));
        assert_eq!(
            (key.error, key.windows, key.decision_grade),
            (None, None, false)
        );
    }
}
