//! The token report (spec §10): the view the TUI renders and `tagteam tokens --json`
//! serializes. Responses are attributed (spec §7) and priced (spec §8.3) here, at query time,
//! so evidence and price overrides recorded later apply to the past.

mod group;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use jiff::civil::Date;
use jiff::tz::TimeZone;
pub use tagteam_core::tokens::{Basis, TokenCounts};
use tagteam_core::tokens::{Prices, day_bounds_ms, month_bounds_ms, month_of};
use tagteam_core::{AccountId, ProviderId};
use tagteam_provider::Provider;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::store::{Store, StoreError};

/// What a report row spans (spec §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenPeriod {
    Day,
    Month,
    Session,
    Block,
}

/// What a period's rows are split by (spec §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenGrouping {
    None,
    Account,
    Model,
    AccountModel,
}

impl TokenGrouping {
    fn by_account(self) -> bool {
        matches!(self, TokenGrouping::Account | TokenGrouping::AccountModel)
    }

    fn by_model(self) -> bool {
        matches!(self, TokenGrouping::Model | TokenGrouping::AccountModel)
    }
}

/// One entry of an account filter. An id matches its account's rows once it is removed too
/// (spec §10.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AccountSel {
    Account(AccountId),
    Unattributed,
}

/// What `token_report` reports (spec §10).
#[derive(Debug, Clone, PartialEq)]
pub struct TokenQuery {
    pub provider: ProviderId,
    pub period: TokenPeriod,
    pub grouping: TokenGrouping,
    /// Inclusive, epoch ms.
    pub since_ms: Option<i64>,
    /// Exclusive, epoch ms.
    pub until_ms: Option<i64>,
    /// `None`: every account and unattributed.
    pub accounts: Option<Vec<AccountSel>>,
    /// `None`: every model; exact names otherwise.
    pub models: Option<Vec<String>>,
    /// Only this session's responses.
    pub session: Option<String>,
    /// Day and month boundaries; the CLI's default is the system zone.
    pub tz: TimeZone,
}

/// The report (spec §10).
#[derive(Debug, Clone, PartialEq)]
pub struct TokenReport {
    pub query: TokenQuery,
    /// The IANA name of `query.tz`, or "UTC".
    pub time_zone: String,
    pub rows: Vec<TokenRow>,
    pub total: TokenTotal,
    pub index: IndexState,
    /// Models with a response that has no price, by name (spec §8.3).
    pub unpriced_models: Vec<String>,
    pub prices_as_of: &'static str,
    /// Models whose price config changed, by name (spec §8.2).
    pub price_overrides: Vec<String>,
    /// The token index's epoch (TR §7.2), `None` when the store has none. Not in the JSON.
    pub tracking_since_ms: Option<i64>,
}

/// One row: a period, split as the query's grouping says (spec §10.1).
#[derive(Debug, Clone, PartialEq)]
pub struct TokenRow {
    pub key: RowKey,
    pub period: PeriodKey,
    /// `Some` when grouped by account.
    pub account: Option<RowAccount>,
    /// `Some` when grouped by model.
    pub model: Option<String>,
    /// Every account present in the row, in report order.
    pub accounts: Vec<RowAccount>,
    /// Every model present in the row, by name.
    pub models: Vec<String>,
    pub counts: TokenCounts,
    pub responses: u64,
    /// Responses whose output is a lower bound (spec §2.3).
    pub unfinalized: u64,
    pub cost: Option<Cost>,
}

/// A row's period (spec §10).
#[derive(Debug, Clone, PartialEq)]
pub enum PeriodKey {
    /// A calendar day in the query's zone.
    Day {
        date: Date,
    },
    Month {
        year: i16,
        month: i8,
    },
    /// `first_ms` and `last_ms` span the row's responses inside the query's range.
    Session {
        session_id: String,
        project: Option<String>,
        first_ms: i64,
        last_ms: i64,
    },
    /// `active` for the block that contains now (spec §9).
    Block {
        start_ms: i64,
        end_ms: i64,
        observed: bool,
        active: Option<ActiveBlock>,
    },
}

impl PeriodKey {
    /// Whether this period's whole span lies before `epoch_ms`, the token index's epoch: its
    /// unattributed rows then predate tracking rather than lost their account. A day or month
    /// spans its calendar interval in `tz`, the end exclusive; a session `first_ms..=last_ms`,
    /// both response times; a block `start_ms..end_ms`. No epoch is never before it.
    pub fn before_tracking(&self, epoch_ms: Option<i64>, tz: &TimeZone) -> bool {
        let Some(epoch) = epoch_ms else {
            return false;
        };
        match self {
            PeriodKey::Day { date } => day_bounds_ms(*date, tz).1 <= epoch,
            PeriodKey::Month { year, month } => month_bounds_ms(*year, *month, tz).1 <= epoch,
            PeriodKey::Session { last_ms, .. } => *last_ms < epoch,
            PeriodKey::Block { end_ms, .. } => *end_ms <= epoch,
        }
    }
}

/// The active block's progress and projection (spec §9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveBlock {
    pub elapsed_s: i64,
    pub remaining_s: i64,
    pub tokens_per_min: f64,
    pub cost_per_hour: Option<Cost>,
    pub projected_tokens: u64,
    pub projected_cost: Option<Cost>,
}

/// An account as a row names it (spec §10).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowAccount {
    Account {
        id: AccountId,
        position: u32,
        label: String,
        alias: Option<String>,
    },
    /// An account no longer in the store; its usage stays (spec §4).
    Removed {
        id: AccountId,
    },
    Unattributed,
}

impl RowAccount {
    /// The filter entry that selects this account.
    fn sel(&self) -> AccountSel {
        match self {
            RowAccount::Account { id, .. } | RowAccount::Removed { id } => {
                AccountSel::Account(id.clone())
            }
            RowAccount::Unattributed => AccountSel::Unattributed,
        }
    }

    /// Spec §10.1's order: accounts by position, then removed accounts, then unattributed.
    fn rank(&self) -> (u8, u32, &str) {
        match self {
            RowAccount::Account { id, position, .. } => (0, *position, id.as_str()),
            RowAccount::Removed { id } => (1, 0, id.as_str()),
            RowAccount::Unattributed => (2, 0, ""),
        }
    }
}

/// A row's identity, stable across refreshes: the TUI keeps a selection by it (spec §10). For
/// example `day:2026-10-14|account:<id>|model:claude-opus-5-5`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RowKey(String);

impl RowKey {
    /// The key of the row for `period`, `account` and `model`: what the report keys its rows
    /// by, public so that a renderer's fixtures key their rows as the report does.
    pub fn new(period: &PeriodKey, account: Option<&RowAccount>, model: Option<&str>) -> Self {
        let mut key = match period {
            PeriodKey::Day { date } => format!("day:{date}"),
            PeriodKey::Month { year, month } => format!("month:{year:04}-{month:02}"),
            PeriodKey::Session { session_id, .. } => format!("session:{session_id}"),
            PeriodKey::Block { start_ms, .. } => format!("block:{start_ms}"),
        };
        if let Some(account) = account {
            key.push_str("|account:");
            key.push_str(match account {
                RowAccount::Account { id, .. } | RowAccount::Removed { id } => id.as_str(),
                RowAccount::Unattributed => "unattributed",
            });
        }
        if let Some(model) = model {
            key.push_str("|model:");
            key.push_str(model);
        }
        RowKey(key)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RowKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// API cost in nano-dollars (spec §8.3). `partial`: some of the responses have no price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    pub usd_nanos: u64,
    pub partial: bool,
}

/// The report's totals (spec §10): those of its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenTotal {
    pub counts: TokenCounts,
    pub responses: u64,
    pub unfinalized: u64,
    pub cost: Option<Cost>,
    /// Total tokens by the basis of their attribution (spec §7).
    pub by_basis: [(Basis, u64); 3],
}

impl Default for TokenTotal {
    fn default() -> Self {
        TokenTotal {
            counts: TokenCounts::default(),
            responses: 0,
            unfinalized: 0,
            cost: None,
            by_basis: [
                (Basis::Profile, 0),
                (Basis::Timeline, 0),
                (Basis::Unattributed, 0),
            ],
        }
    }
}

/// How current the index is (spec §10).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexState {
    /// The last completed refresh; `None` while the index is partial (spec §14).
    pub refreshed_at_ms: Option<i64>,
    /// The latest of `token_state.changed_at` and every evidence row's `recorded_at`
    /// (spec §6.4): it moves when a report may have changed.
    pub changed_at_ms: Option<i64>,
    /// As of the last completed refresh.
    pub files: u64,
    /// The index's own count, which every committed batch moves, so a cold index shows its
    /// progress.
    pub responses: u64,
    /// In the last completed refresh.
    pub skipped_lines: u64,
    pub unreadable_files: u64,
    pub oldest_ms: Option<i64>,
    pub newest_ms: Option<i64>,
}

impl TokenQuery {
    /// The grouping the report applies (spec §10.1): the requested one, except that a block
    /// report is always per account, so `none` becomes `account` and `model` becomes
    /// `account,model`.
    pub fn applied_grouping(&self) -> TokenGrouping {
        group::effective_grouping(self.period, self.grouping)
    }

    /// The query one level down from `row` (spec §10.1), or `None` at a session's per-model
    /// rows. From a day, month or block row: the sessions over its span, filtered to its
    /// account and model when grouped by them. A day or month row counts only what lies in the
    /// range, so its span is clipped to it; a block row counts its whole block. From a session
    /// row: that session over its whole life, by account and model.
    pub fn drill(&self, row: &TokenRow) -> Option<TokenQuery> {
        let clip = |(start, end): (i64, i64)| {
            (
                self.since_ms.map_or(start, |s| s.max(start)),
                self.until_ms.map_or(end, |u| u.min(end)),
            )
        };
        let (since, until) = match &row.period {
            PeriodKey::Session { session_id, .. } => {
                if self.session.is_some() && self.grouping == TokenGrouping::AccountModel {
                    return None;
                }
                return Some(TokenQuery {
                    period: TokenPeriod::Session,
                    grouping: TokenGrouping::AccountModel,
                    since_ms: None,
                    until_ms: None,
                    session: Some(session_id.clone()),
                    ..self.clone()
                });
            }
            PeriodKey::Day { date } => clip(day_bounds_ms(*date, &self.tz)),
            PeriodKey::Month { year, month } => clip(month_bounds_ms(*year, *month, &self.tz)),
            PeriodKey::Block {
                start_ms, end_ms, ..
            } => (*start_ms, *end_ms),
        };
        Some(TokenQuery {
            period: TokenPeriod::Session,
            since_ms: Some(since),
            until_ms: Some(until),
            accounts: match &row.account {
                Some(account) => Some(vec![account.sel()]),
                None => self.accounts.clone(),
            },
            models: match &row.model {
                Some(model) => Some(vec![model.clone()]),
                None => self.models.clone(),
            },
            ..self.clone()
        })
    }
}

/// Spec §11.1: how far back a report's range starts by default, for every period but `month`.
const DEFAULT_SINCE_MS: i64 = 30 * 86_400_000;

/// Spec §11.1's default `since_ms`, a function of the time and the zone alone: 30 days back, or
/// for `month` the first day of the eleventh month before `now_ms`'s, at midnight in `tz`, so
/// the report spans twelve calendar months with this one. `tagteam tokens` and the TUI both
/// take it (TUI spec §5.3).
pub fn default_since_ms(period: &TokenPeriod, now_ms: i64, tz: &TimeZone) -> i64 {
    if !matches!(period, TokenPeriod::Month) {
        return now_ms.saturating_sub(DEFAULT_SINCE_MS);
    }
    let (year, month) = month_of(now_ms, tz);
    let index = i32::from(year) * 12 + i32::from(month) - 1 - 11;
    let (year, month) = (index.div_euclid(12), index.rem_euclid(12) + 1);
    month_bounds_ms(year as i16, month as i8, tz).0
}

impl Engine {
    /// Spec §10: the report `q` asks for, from the index and the evidence as they stand. It
    /// reads only: it never refreshes the index (spec §5) and never creates the store, without
    /// which the report is empty. A block report reports each block that meets the range
    /// whole, and its total is its rows'.
    ///
    /// Spec §7.2: the usage is read first, then the pending launch files, then the stored
    /// launches, each read begun after the one before. `run` has its launch in the store or in
    /// a pending file before it spawns the child, and a file goes only once its launch is
    /// stored. So every response the usage holds has its launch in a file when the files are
    /// read, or in the store when the store is. In another order a launch can be in neither
    /// read: one kept in a file written after the files were read, or one a scan moved from its
    /// file to the store between the two launch reads. Its responses would then go to the
    /// default home's account.
    pub fn token_report(&self, q: &TokenQuery) -> Result<TokenReport, EngineError> {
        let p = provider_with_transcripts(self, &q.provider)?;
        let table = p.price_table();
        // The query provider's own overrides, whichever provider the settings were loaded for
        // (spec §8.2).
        let none = BTreeMap::new();
        let settings = self.settings();
        let overrides = settings.prices.get(&q.provider).unwrap_or(&none);
        let (prices, price_overrides) = Prices::new(table, overrides);
        let mut report = TokenReport {
            query: q.clone(),
            time_zone: q.tz.iana_name().unwrap_or("UTC").to_owned(),
            rows: Vec::new(),
            total: TokenTotal::default(),
            index: IndexState::default(),
            unpriced_models: Vec::new(),
            prices_as_of: table.as_of,
            price_overrides,
            tracking_since_ms: None,
        };
        let Some(store) = self.existing_store()? else {
            return Ok(report);
        };
        // Read before the usage, so whatever lands after it moves `changed_at_ms` again.
        report.index = index_state(&store, &q.provider)?;
        let epoch = store.token_epoch()?;
        report.tracking_since_ms = epoch.map(|e| e.at);
        // `Some` of an empty list matches nothing (spec §10.1).
        if q.accounts.as_ref().is_some_and(Vec::is_empty)
            || q.models.as_ref().is_some_and(Vec::is_empty)
        {
            return Ok(report);
        }
        // The usage, the pending files, then the stored launches, as the doc says. Each store
        // read is a statement of its own: a transaction opened before the files are read would
        // fix the launches' snapshot there. A test stops after each read. The epoch read above
        // bounds the starts the pending files' names give.
        let usage = group::usage(q, &store)?;
        hooks::point(self, "report-usage-read")?;
        // Launches `run` kept in a pending file count as stored ones, read only.
        let pending = self.pending_evidence(p.as_ref(), epoch.map(|e| e.at));
        hooks::point(self, "report-pending-read")?;
        let launches = store.launches(&q.provider)?;
        hooks::point(self, "report-launches-read")?;
        let reads = group::Reads {
            epoch,
            usage,
            pending,
            launches,
        };
        let grouped = group::group(q, p.as_ref(), &store, &reads, &prices, self.now_ms())?;
        report.rows = grouped.rows;
        report.total = grouped.total;
        report.unpriced_models = grouped.unpriced_models;
        Ok(report)
    }

    /// The `index` part of a report alone (spec §10), for deciding whether to re-query: a few
    /// small reads, and none without a store, which it never creates.
    pub fn token_index_state(&self, provider: &ProviderId) -> Result<IndexState, EngineError> {
        provider_with_transcripts(self, provider)?;
        match self.existing_store()? {
            Some(store) => Ok(index_state(&store, provider)?),
            None => Ok(IndexState::default()),
        }
    }
}

/// `id`'s provider, which must read transcripts (spec §3.2).
fn provider_with_transcripts(
    engine: &Engine,
    id: &ProviderId,
) -> Result<Arc<dyn Provider>, EngineError> {
    let p = engine.provider(id)?;
    if !p.capabilities().transcripts {
        return Err(EngineError::InvalidInput(format!(
            "{} has no transcripts",
            p.display_name()
        )));
    }
    Ok(p)
}

fn index_state(store: &Store, provider: &ProviderId) -> Result<IndexState, StoreError> {
    let state = store.token_state(provider)?;
    let evidence = store.evidence_changed_at(provider)?;
    let (oldest_ms, newest_ms) = store.token_span(provider)?;
    let stats = state.as_ref().map(|s| s.stats).unwrap_or_default();
    Ok(IndexState {
        refreshed_at_ms: state.as_ref().and_then(|s| s.refreshed_at),
        changed_at_ms: state.as_ref().and_then(|s| s.changed_at).max(evidence),
        files: stats.files,
        responses: stats.responses,
        skipped_lines: stats.skipped_lines,
        unreadable_files: stats.unreadable_files,
        oldest_ms,
        newest_ms,
    })
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use tagteam_core::time::{format_iso8601, parse_iso8601_ms};

    use super::*;

    /// `ms` as an ISO 8601 UTC time, to the second.
    fn iso(ms: i64) -> String {
        format_iso8601(ms.div_euclid(1000))
    }

    /// `s`, an ISO 8601 time, in epoch ms.
    fn at(s: &str) -> i64 {
        parse_iso8601_ms(s).unwrap()
    }

    #[test]
    fn default_ranges_follow_the_zones_calendar() {
        // Spec §11.1, at fixed instants: 30 days back for every period but month, and for
        // month the first of the eleventh month back, at midnight where the report's days are.
        let utc = TimeZone::UTC;
        let brussels = TimeZone::get("Europe/Brussels").unwrap();
        let since = |period: TokenPeriod, now: &str, tz: &TimeZone| {
            iso(default_since_ms(&period, at(now), tz))
        };
        let now = "2026-09-21T14:13:20Z";
        assert_eq!(since(TokenPeriod::Day, now, &utc), "2026-08-22T14:13:20Z");
        assert_eq!(
            since(TokenPeriod::Block, now, &brussels),
            "2026-08-22T14:13:20Z",
            "30 days of 24 hours, whatever the zone"
        );
        assert_eq!(since(TokenPeriod::Month, now, &utc), "2025-10-01T00:00:00Z");
        // Across the turn of a month and of a year.
        assert_eq!(
            since(TokenPeriod::Month, "2026-02-28T23:59:59.999Z", &utc),
            "2025-03-01T00:00:00Z"
        );
        assert_eq!(
            since(TokenPeriod::Month, "2026-03-01T00:00:00Z", &utc),
            "2025-04-01T00:00:00Z"
        );
        assert_eq!(
            since(TokenPeriod::Month, "2026-01-15T12:00:00Z", &utc),
            "2025-02-01T00:00:00Z"
        );
        // 23:30 UTC on 31 October is already 1 November in Brussels, on winter time (UTC+1):
        // the range starts on 1 December 2025 at its midnight there.
        assert_eq!(
            since(TokenPeriod::Month, "2026-10-31T23:30:00Z", &brussels),
            "2025-11-30T23:00:00Z"
        );
        assert_eq!(
            since(TokenPeriod::Month, "2026-10-31T23:30:00Z", &utc),
            "2025-11-01T00:00:00Z",
            "still October in UTC"
        );
        // From late March 2026, eleven months back is April 2025, on summer time (UTC+2).
        assert_eq!(
            since(TokenPeriod::Month, "2026-03-29T12:00:00Z", &brussels),
            "2025-03-31T22:00:00Z"
        );
    }

    #[test]
    fn a_period_is_before_tracking_only_when_its_whole_span_ends_by_the_epoch() {
        let tz = TimeZone::get("Europe/Brussels").unwrap();
        let (start_23, _) = day_bounds_ms(date(2026, 10, 23), &tz);
        let epoch = start_23 + 9 * 3_600_000; // 09:00 on the 23rd
        let day = |d| PeriodKey::Day { date: d };
        assert!(day(date(2026, 10, 22)).before_tracking(Some(epoch), &tz));
        assert!(!day(date(2026, 10, 23)).before_tracking(Some(epoch), &tz));
        assert!(!day(date(2026, 10, 22)).before_tracking(None, &tz));
        let month = |m| PeriodKey::Month {
            year: 2026,
            month: m,
        };
        assert!(month(9).before_tracking(Some(epoch), &tz));
        assert!(!month(10).before_tracking(Some(epoch), &tz));
        let session = |last_ms| PeriodKey::Session {
            session_id: "s".into(),
            project: None,
            first_ms: start_23,
            last_ms,
        };
        assert!(session(epoch - 1).before_tracking(Some(epoch), &tz));
        assert!(!session(epoch).before_tracking(Some(epoch), &tz)); // a response at the epoch
        let block = |end_ms| PeriodKey::Block {
            start_ms: end_ms - 5 * 3_600_000,
            end_ms,
            observed: true,
            active: None,
        };
        assert!(block(epoch).before_tracking(Some(epoch), &tz));
        assert!(!block(epoch + 1).before_tracking(Some(epoch), &tz));
    }

    #[test]
    fn a_dst_day_before_the_epoch_ends_at_its_zone_s_midnight() {
        // 2026-10-25 is 25 hours long in Europe/Brussels.
        let tz = TimeZone::get("Europe/Brussels").unwrap();
        let (start, end) = day_bounds_ms(date(2026, 10, 25), &tz);
        assert_eq!(end - start, 25 * 3_600_000);
        let d = PeriodKey::Day {
            date: date(2026, 10, 25),
        };
        assert!(d.before_tracking(Some(end), &tz));
        assert!(!d.before_tracking(Some(end - 1), &tz));
    }
}
