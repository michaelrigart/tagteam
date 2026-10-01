use tagteam_core::pace::pace;
use tagteam_core::trust::decision_grade;
use tagteam_core::usage::earliest_relevant_reset;
use tagteam_core::{AccountId, Pace, ProviderId, Sample, TrustInputs, Window};
use tagteam_provider::{KindTraits, Read};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::{AccountRow, Store, UsageStateRow};

/// §8.7: pace and projections read the samples of the 48 h before a reading.
const PACE_LOOKBACK_S: i64 = 48 * 3600;

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
    /// The last good reading, each window with its pace (§8.7).
    pub windows: Option<Vec<(Window, Pace)>>,
    /// Whether the reading may drive a decision and be shown as `usage` (§8.4).
    pub decision_grade: bool,
    pub fetched_at: Option<i64>,
    pub age_s: Option<i64>,
    /// `last_error` (or `no-data`) when the status is `Unavailable`.
    pub error: Option<String>,
    /// When the next fetch may happen, `max(backoff_until, next_poll_at)`, when `Unavailable`.
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
fn with_pace(
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

impl Engine {
    /// §13.2's usage for one account: its status (Decision 10), its last good reading with
    /// pace, and whether that reading is decision-grade (§8.4). Reads the store only.
    fn usage_view(
        &self,
        store: Option<&Store>,
        row: &AccountRow,
        kind: &KindTraits,
        supported: bool,
    ) -> Result<UsageView, EngineError> {
        let state = match store {
            Some(s) if supported => s.usage_state(&row.id)?,
            _ => None,
        };
        let status = usage_status(supported, kind, row, state.as_ref());
        let (Some(store), Some(state)) = (store, state) else {
            return Ok(UsageView::unread(status, None));
        };
        let now_ms = self.now_ms();
        let now_s = now_ms.div_euclid(1000);
        let windows = match state.fetched_at {
            Some(at) => {
                let read = state.last_good.as_deref().unwrap_or_default();
                Some(with_pace(store, &row.id, read, at)?)
            }
            None => None,
        };
        let reset = state
            .last_good
            .as_deref()
            .and_then(|w| earliest_relevant_reset(w, &self.settings().models));
        let trusted = windows.is_some()
            && decision_grade(&TrustInputs {
                now_s,
                fetched_at: state.fetched_at,
                consecutive_failures: state.consecutive_failures,
                plan_in_force: state.next_poll_at.is_some_and(|at| at > now_s),
                live_lease: store.usage_lease_live(&row.id, now_ms)?,
                last_429_at: state.last_429_at,
                earliest_relevant_reset: reset,
            });
        let unavailable = status == UsageStatus::Unavailable;
        let error = unavailable.then(|| {
            state
                .last_error
                .clone()
                .filter(|_| state.consecutive_failures > 0)
                .unwrap_or_else(|| NO_DATA.to_owned())
        });
        Ok(UsageView {
            status,
            windows,
            decision_grade: trusted,
            fetched_at: state.fetched_at,
            age_s: state.fetched_at.map(|at| (now_s - at).max(0)),
            error,
            retry_at: if unavailable {
                state.backoff_until.max(state.next_poll_at)
            } else {
                None
            },
        })
    }

    fn view(&self, provider: &ProviderId) -> Result<(ProviderAccounts, Read<String>), EngineError> {
        let p = self.provider(provider)?;
        let store = self.existing_store()?;
        let rows = match &store {
            Some(s) => s.accounts(provider)?,
            None => vec![],
        };
        let live = p.live_identity(&self.env);
        let live_key = live.as_ref().map(|i| p.identity_key(i).as_str().to_owned());
        let stored_active = match (&live_key, &store) {
            (Read::Unreadable(_), Some(s)) => s.active(provider)?,
            _ => None,
        };
        let supported = p.capabilities().usage;
        let accounts = rows
            .into_iter()
            .map(|row| {
                let active = match &live_key {
                    Read::Present(k) => &row.identity_key == k,
                    Read::Absent => false,
                    Read::Unreadable(_) => stored_active.as_ref() == Some(&row.id),
                };
                let kind = p.kind_traits(&row.kind);
                let usage = self.usage_view(store.as_deref(), &row, &kind, supported)?;
                Ok(AccountView {
                    kind,
                    row,
                    active,
                    usage,
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        let active_position = accounts.iter().find(|v| v.active).map(|v| v.row.position);
        let live_label = live.map(|i| i.email.unwrap_or(i.label));
        Ok((
            ProviderAccounts {
                provider: provider.clone(),
                active_position,
                accounts,
            },
            live_label,
        ))
    }

    /// A row as the views show it, for a caller that already knows whether it is active. Its
    /// usage comes from the store; a store that cannot be read leaves it unread, logged, since
    /// the callers report a change that has already happened.
    pub fn account_view(&self, row: AccountRow, active: bool) -> AccountView {
        let provider = self.registry.get(&row.provider);
        let kind = provider
            .as_ref()
            .map_or(UNREGISTERED, |p| p.kind_traits(&row.kind));
        let supported = provider.is_some_and(|p| p.capabilities().usage);
        let usage = self
            .existing_store()
            .and_then(|s| self.usage_view(s.as_deref(), &row, &kind, supported))
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
