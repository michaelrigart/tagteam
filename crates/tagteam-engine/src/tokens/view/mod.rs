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
use tagteam_core::tokens::{Prices, day_bounds_ms, month_bounds_ms};
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
    fn new(period: &PeriodKey, account: Option<&RowAccount>, model: Option<&str>) -> Self {
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
        let overrides = self.settings().prices.get(&q.provider).unwrap_or(&none);
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
        };
        let Some(store) = self.existing_store()? else {
            return Ok(report);
        };
        // Read before the usage, so whatever lands after it moves `changed_at_ms` again.
        report.index = index_state(&store, &q.provider)?;
        // `Some` of an empty list matches nothing (spec §10.1).
        if q.accounts.as_ref().is_some_and(Vec::is_empty)
            || q.models.as_ref().is_some_and(Vec::is_empty)
        {
            return Ok(report);
        }
        // The usage, the pending files, then the stored launches, as the doc says. Each store
        // read is a statement of its own: a transaction opened before the files are read would
        // fix the launches' snapshot there. A test stops after each read. The epoch bounds the
        // starts the pending files' names give.
        let epoch = store.token_epoch()?;
        let usage = group::usage(q, &store)?;
        hooks::point(self, "report-usage-read")?;
        // Launches `run` kept in a pending file count as stored ones, read only.
        let pending = self
            .pending_launches(p.as_ref(), epoch.map(|e| e.at))
            .into_iter()
            .map(|(_, launch)| launch)
            .collect();
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
