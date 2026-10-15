//! The report's rows (spec §10.1): every response the query reads, attributed (spec §7),
//! priced (spec §8.3), filtered, and summed by period, account and model.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};

use jiff::civil::Date;
use jiff::tz::TimeZone;
use tagteam_core::tokens::{
    ActiveChange, BLOCK_MS, BlockSpan, Coverage, Home, Launch, Launches, Prices, Projection,
    Sighting, Timeline, TimelineEvent, assign_blocks, attribute, day_bounds_ms, day_of,
    month_bounds_ms, month_of, observed_blocks, place, project,
};
use tagteam_core::{AccountId, ProviderId, WindowKind};
use tagteam_provider::Provider;

use super::{
    AccountSel, ActiveBlock, Basis, Cost, PeriodKey, RowAccount, RowKey, TokenCounts,
    TokenGrouping, TokenPeriod, TokenQuery, TokenRow, TokenTotal,
};
use crate::store::{AccountRow, EventRow, Store, StoreError, UsageRow};

/// How far before `since` a block report reads (spec §9). A block that straddles `since` is
/// then whole, and the chain of inferred blocks leading to it starts after a quiet gap of five
/// hours, which a day nearly always holds, so a `since` that moves leaves block starts alone.
const BLOCK_LOOKBACK_MS: i64 = 24 * 3_600_000;

/// What `group` gives the report.
pub(super) struct Grouped {
    pub(super) rows: Vec<TokenRow>,
    pub(super) total: TokenTotal,
    pub(super) unpriced_models: Vec<String>,
}

/// The rows and totals of `q` (spec §10.1). `now_ms` decides the active block.
pub(super) fn group(
    q: &TokenQuery,
    p: &dyn Provider,
    store: &Store,
    prices: &Prices,
    now_ms: i64,
) -> Result<Grouped, StoreError> {
    let by_block = q.period == TokenPeriod::Block;
    // A block is a span of its account's whole usage (spec §9), so a block report resolves
    // every response it reads; the filters only choose which of them count.
    let usage = if by_block {
        store.token_usage(
            &q.provider,
            q.since_ms.map(|s| s.saturating_sub(BLOCK_LOOKBACK_MS)),
            q.until_ms.map(|u| u.saturating_add(BLOCK_MS)),
            None,
        )?
    } else {
        store.token_usage(&q.provider, q.since_ms, q.until_ms, q.session.as_deref())?
    };
    let accounts = store.accounts(&q.provider)?;
    let mut evidence = Evidence::load(store, &q.provider, &accounts)?;
    let mut t = Tables {
        slots: Slots::new(&accounts, q.accounts.as_deref()),
        models: Models::new(q.models.as_deref()),
        sessions: Sessions::default(),
        blocks: Vec::new(),
    };
    let mut resolved = Vec::with_capacity(usage.len());
    for (i, row) in usage.iter().enumerate() {
        let model = t.models.intern(&row.model);
        let chosen =
            t.models.allowed[model] && q.session.as_deref().is_none_or(|s| s == row.session_id);
        if !chosen && !by_block {
            continue;
        }
        let session = t.sessions.intern(row, &mut evidence.sightings);
        let (account, basis) = evidence.account_of(&t.sessions.list[session].sightings, row.at_ms);
        let slot = t.slots.of(account);
        let wanted = chosen && t.slots.allowed[slot];
        if wanted || by_block {
            resolved.push(Resolved {
                row: i,
                model,
                session,
                slot,
                basis,
                wanted,
            });
        }
    }
    let block_of = if by_block {
        let (blocks, block_of) = series_blocks(p, store, &t.slots, &usage, &resolved)?;
        t.blocks = blocks;
        block_of
    } else {
        Vec::new()
    };
    let grouping = effective_grouping(q.period, q.grouping);
    let mut calendar = Calendar {
        tz: &q.tz,
        months: q.period == TokenPeriod::Month,
        last: None,
    };
    let mut groups: HashMap<GroupKey, Acc> = HashMap::new();
    let mut total = Acc::new();
    let mut by_basis = [0u64; 3];
    for (i, r) in resolved.iter().enumerate() {
        if !r.wanted {
            continue;
        }
        let row = &usage[r.row];
        let period = match q.period {
            TokenPeriod::Day | TokenPeriod::Month => calendar.period(row.at_ms),
            TokenPeriod::Session => Period::Session(r.session),
            TokenPeriod::Block => {
                let b = block_of[i];
                if !meets_range(&t.blocks[b].span, q) {
                    continue;
                }
                Period::Block(b)
            }
        };
        let cost = prices.cost_nanos(&row.model, &row.counts, row.speed.as_deref());
        if cost.is_none() {
            t.models.unpriced[r.model] = true;
        }
        let key = GroupKey {
            period,
            slot: grouping.by_account().then_some(r.slot),
            model: grouping.by_model().then_some(r.model),
        };
        groups
            .entry(key)
            .or_insert_with(Acc::new)
            .add(row, cost, r.slot, r.model);
        total.add(row, cost, r.slot, r.model);
        let basis = &mut by_basis[basis_index(r.basis)];
        *basis = basis.saturating_add(row.counts.total());
        let session = &mut t.sessions.list[r.session];
        session.last_ms = session.last_ms.max(row.at_ms);
    }
    let mut unpriced_models: Vec<String> = t
        .models
        .names
        .iter()
        .zip(&t.models.unpriced)
        .filter(|(_, unpriced)| **unpriced)
        .map(|(name, _)| name.clone())
        .collect();
    unpriced_models.sort();
    Ok(Grouped {
        rows: t.rows(groups, now_ms),
        total: TokenTotal {
            counts: total.counts,
            responses: total.responses,
            unfinalized: total.unfinalized,
            cost: total.cost(),
            by_basis: [
                (Basis::Profile, by_basis[0]),
                (Basis::Timeline, by_basis[1]),
                (Basis::Unattributed, by_basis[2]),
            ],
        },
        unpriced_models,
    })
}

/// Spec §10.1: blocks are per account, so a block report is always grouped by account.
pub(super) fn effective_grouping(period: TokenPeriod, grouping: TokenGrouping) -> TokenGrouping {
    match (period, grouping) {
        (TokenPeriod::Block, TokenGrouping::None) => TokenGrouping::Account,
        (TokenPeriod::Block, TokenGrouping::Model) => TokenGrouping::AccountModel,
        (_, grouping) => grouping,
    }
}

fn meets_range(span: &BlockSpan, q: &TokenQuery) -> bool {
    q.since_ms.is_none_or(|s| span.end_ms > s) && q.until_ms.is_none_or(|u| span.start_ms < u)
}

fn basis_index(basis: Basis) -> usize {
    match basis {
        Basis::Profile => 0,
        Basis::Timeline => 1,
        Basis::Unattributed => 2,
    }
}

/// What places a response (spec §7), read once per report.
struct Evidence {
    /// By session; each session's are taken out when the report first meets it.
    sightings: HashMap<String, Vec<Sighting>>,
    launches: Launches,
    coverage: Coverage,
    timeline: Timeline,
}

impl Evidence {
    fn load(
        store: &Store,
        provider: &ProviderId,
        accounts: &[AccountRow],
    ) -> Result<Self, StoreError> {
        let mut sightings: HashMap<String, Vec<Sighting>> = HashMap::new();
        for s in store.sightings(provider)? {
            let home = if s.home.is_empty() {
                Home::Default
            } else {
                Home::Profile(AccountId::from_string(s.home))
            };
            sightings.entry(s.session_id).or_default().push(Sighting {
                home,
                first_ms: s.first_at,
                last_ms: s.last_at,
            });
        }
        let launches = Launches::new(
            store
                .launches(provider)?
                .into_iter()
                .map(|l| Launch {
                    account: AccountId::from_string(l.account_id),
                    start_ms: l.started_at,
                    end_ms: l.ended_at,
                })
                .collect(),
        );
        // No epoch row (after a full purge, before the next refresh completes): every time is
        // before it, and coverage is never complete.
        let epoch = store.token_epoch()?;
        let epoch_ms = epoch.map_or(i64::MAX, |e| e.at);
        let events: Vec<TimelineEvent> = store
            .events()?
            .into_iter()
            .filter(|e| &e.provider == provider)
            .filter_map(timeline_event)
            .collect();
        let coverage = Coverage {
            epoch_ms,
            seeded: epoch.is_some_and(|e| e.seeded),
            last_pre_epoch_remove_ms: events
                .iter()
                .filter_map(|e| match e {
                    TimelineEvent::Remove { at_ms, .. } if *at_ms < epoch_ms => Some(*at_ms),
                    _ => None,
                })
                .max(),
        };
        let history: Vec<ActiveChange> = store
            .active_history(provider)?
            .into_iter()
            .map(|(at_ms, account)| ActiveChange {
                at_ms,
                account: account.map(AccountId::from_string),
            })
            .collect();
        let added_at: BTreeMap<AccountId, i64> = accounts
            .iter()
            .map(|a| (a.id.clone(), a.added_at))
            .collect();
        Ok(Evidence {
            sightings,
            launches,
            coverage,
            timeline: Timeline::build(&events, &history, epoch_ms, &added_at),
        })
    }

    /// The account a response at `t` of a session with `sightings` went to, and on what basis
    /// (spec §7).
    fn account_of(&self, sightings: &[Sighting], t: i64) -> (Option<AccountId>, Basis) {
        let placement = place(sightings, &self.launches, &self.coverage, t);
        attribute(&placement, &self.timeline, t)
    }
}

/// An `events` row as the default home's timeline reads it (spec §7.3): `switch` and
/// `switch-recovered` move the login to `to_id`, `add` and `import` start an account, and
/// `remove`, which records the account as `from_id`, ends it.
fn timeline_event(e: EventRow) -> Option<TimelineEvent> {
    match e.kind.as_str() {
        "switch" | "switch-recovered" => Some(TimelineEvent::Switch {
            at_ms: e.at,
            from: e.from_id,
            to: e.to_id?,
        }),
        "add" | "import" => Some(TimelineEvent::Add {
            at_ms: e.at,
            account: e.to_id?,
        }),
        "remove" => Some(TimelineEvent::Remove {
            at_ms: e.at,
            account: e.from_id?,
        }),
        _ => None,
    }
}

/// One response as the report resolved it: indices into its tables, and its attribution.
#[derive(Clone, Copy)]
struct Resolved {
    row: usize,
    model: usize,
    session: usize,
    slot: usize,
    basis: Basis,
    /// It passes the query's filters.
    wanted: bool,
}

/// What the indices of a report's rows point at.
struct Tables<'q> {
    slots: Slots<'q>,
    models: Models<'q>,
    sessions: Sessions,
    blocks: Vec<Block>,
}

/// The accounts a report names: the provider's, then each removed account a response went
/// to, and the unattributed bucket.
struct Slots<'q> {
    accounts: Vec<RowAccount>,
    by_id: HashMap<AccountId, usize>,
    unattributed: usize,
    filter: Option<&'q [AccountSel]>,
    /// Whether the query's account filter selects it.
    allowed: Vec<bool>,
}

impl<'q> Slots<'q> {
    fn new(rows: &[AccountRow], filter: Option<&'q [AccountSel]>) -> Self {
        let mut slots = Slots {
            accounts: Vec::new(),
            by_id: HashMap::new(),
            unattributed: 0,
            filter,
            allowed: Vec::new(),
        };
        for row in rows {
            slots.push(RowAccount::Account {
                id: row.id.clone(),
                position: row.position,
                label: row.label.clone(),
                alias: row.alias.clone(),
            });
        }
        slots.unattributed = slots.push(RowAccount::Unattributed);
        slots
    }

    fn push(&mut self, account: RowAccount) -> usize {
        let slot = self.accounts.len();
        self.allowed
            .push(self.filter.is_none_or(|f| f.contains(&account.sel())));
        if let RowAccount::Account { id, .. } | RowAccount::Removed { id } = &account {
            self.by_id.insert(id.clone(), slot);
        }
        self.accounts.push(account);
        slot
    }

    /// The slot of a response attributed to `account`: an id the store no longer holds is a
    /// removed account (spec §10).
    fn of(&mut self, account: Option<AccountId>) -> usize {
        let Some(id) = account else {
            return self.unattributed;
        };
        match self.by_id.get(&id) {
            Some(&slot) => slot,
            None => self.push(RowAccount::Removed { id }),
        }
    }
}

/// The models a report's responses name.
struct Models<'q> {
    names: Vec<String>,
    by_name: HashMap<String, usize>,
    filter: Option<&'q [String]>,
    /// Whether the query's model filter selects it.
    allowed: Vec<bool>,
    /// Whether a response of it that counts has no price (spec §8.3).
    unpriced: Vec<bool>,
}

impl<'q> Models<'q> {
    fn new(filter: Option<&'q [String]>) -> Self {
        Models {
            names: Vec::new(),
            by_name: HashMap::new(),
            filter,
            allowed: Vec::new(),
            unpriced: Vec::new(),
        }
    }

    fn intern(&mut self, name: &str) -> usize {
        if let Some(&model) = self.by_name.get(name) {
            return model;
        }
        let model = self.names.len();
        self.names.push(name.to_owned());
        self.by_name.insert(name.to_owned(), model);
        self.allowed
            .push(self.filter.is_none_or(|f| f.iter().any(|m| m == name)));
        self.unpriced.push(false);
        model
    }
}

/// The sessions a report's responses belong to.
#[derive(Default)]
struct Sessions {
    list: Vec<Session>,
    by_id: HashMap<String, usize>,
}

struct Session {
    id: String,
    project: Option<String>,
    sightings: Vec<Sighting>,
    /// Its latest response that counts, which orders session rows (spec §10.1).
    last_ms: i64,
}

impl Sessions {
    fn intern(&mut self, row: &UsageRow, sightings: &mut HashMap<String, Vec<Sighting>>) -> usize {
        if let Some(&session) = self.by_id.get(row.session_id.as_str()) {
            return session;
        }
        let session = self.list.len();
        self.list.push(Session {
            id: row.session_id.clone(),
            project: row.project.clone(),
            sightings: sightings
                .remove(row.session_id.as_str())
                .unwrap_or_default(),
            last_ms: i64::MIN,
        });
        self.by_id.insert(row.session_id.clone(), session);
        session
    }
}

/// One block of one series (spec §9).
struct Block {
    span: BlockSpan,
    /// Its first response, of the account's whole usage: where the active block's burn rate
    /// starts, whatever the filters keep.
    first_ms: i64,
}

/// Spec §9: each series' blocks, an account's from its observed resets and its responses,
/// a removed account's and the unattributed series' from their responses alone, each with its
/// first response. Returns the blocks and, for each resolved response, its block. A series with
/// no response that counts gets none.
fn series_blocks(
    p: &dyn Provider,
    store: &Store,
    slots: &Slots<'_>,
    usage: &[UsageRow],
    resolved: &[Resolved],
) -> Result<(Vec<Block>, Vec<usize>), StoreError> {
    let mut series: Vec<Vec<usize>> = vec![Vec::new(); slots.accounts.len()];
    for (i, r) in resolved.iter().enumerate() {
        series[r.slot].push(i);
    }
    let mut short: HashMap<String, bool> = HashMap::new();
    let mut blocks = Vec::new();
    let mut block_of = vec![0; resolved.len()];
    for (slot, entries) in series.iter().enumerate() {
        if !entries.iter().any(|&i| resolved[i].wanted) {
            continue;
        }
        let times: Vec<i64> = entries
            .iter()
            .map(|&i| usage[resolved[i].row].at_ms)
            .collect();
        let observed = match &slots.accounts[slot] {
            RowAccount::Account { id, .. } => {
                observed_blocks(&short_resets(p, store, id, times[0], &mut short)?)
            }
            // A removed account's samples went with it (core §6.1).
            RowAccount::Removed { .. } | RowAccount::Unattributed => Vec::new(),
        };
        let (spans, of) = assign_blocks(&observed, &times);
        let base = blocks.len();
        blocks.extend(spans.into_iter().map(|span| Block {
            span,
            first_ms: i64::MAX,
        }));
        for ((&i, &at), b) in entries.iter().zip(&times).zip(of) {
            let block = &mut blocks[base + b];
            block.first_ms = block.first_ms.min(at);
            block_of[i] = base + b;
        }
    }
    Ok((blocks, block_of))
}

/// The resets, in ms, of `id`'s samples of the provider's short window (spec §9) taken from
/// two blocks before `from_ms` on: early enough for every block that can hold a response at
/// or after it, and for the block before that, which may move its start.
fn short_resets(
    p: &dyn Provider,
    store: &Store,
    id: &AccountId,
    from_ms: i64,
    short: &mut HashMap<String, bool>,
) -> Result<Vec<i64>, StoreError> {
    let since_s = from_ms.saturating_sub(2 * BLOCK_MS).div_euclid(1000);
    let mut resets = Vec::new();
    for (key, sample) in store.usage_samples(id, None, since_s)? {
        let is_short = *short.entry(key).or_insert_with_key(|key| {
            p.describe_window(key)
                .is_some_and(|w| w.kind == WindowKind::Short)
        });
        if let Some(reset) = sample.resets_at.filter(|_| is_short) {
            resets.push(reset.saturating_mul(1000));
        }
    }
    resets.sort_unstable();
    resets.dedup();
    Ok(resets)
}

/// A row's period, by the report's own indices.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Period {
    Day(Date),
    Month(i16, i8),
    Session(usize),
    Block(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GroupKey {
    period: Period,
    slot: Option<usize>,
    model: Option<usize>,
}

/// The calendar day or month of a time in the query's zone (spec §10.1). Responses come in
/// time order, so the bounds of the last period found answer most of them without a zone
/// conversion.
struct Calendar<'q> {
    tz: &'q TimeZone,
    months: bool,
    last: Option<(i64, i64, Period)>,
}

impl Calendar<'_> {
    fn period(&mut self, t: i64) -> Period {
        if let Some((_, _, period)) = self.last.filter(|&(start, end, _)| start <= t && t < end) {
            return period;
        }
        let (period, (start, end)) = if self.months {
            let (year, month) = month_of(t, self.tz);
            (
                Period::Month(year, month),
                month_bounds_ms(year, month, self.tz),
            )
        } else {
            let date = day_of(t, self.tz);
            (Period::Day(date), day_bounds_ms(date, self.tz))
        };
        self.last = Some((start, end, period));
        period
    }
}

/// One row's sums while the report groups.
struct Acc {
    counts: TokenCounts,
    responses: u64,
    unfinalized: u64,
    cost_nanos: u64,
    unpriced: u64,
    first_ms: i64,
    last_ms: i64,
    slots: Vec<usize>,
    models: Vec<usize>,
}

impl Acc {
    fn new() -> Self {
        Acc {
            counts: TokenCounts::default(),
            responses: 0,
            unfinalized: 0,
            cost_nanos: 0,
            unpriced: 0,
            first_ms: i64::MAX,
            last_ms: i64::MIN,
            slots: Vec::new(),
            models: Vec::new(),
        }
    }

    fn add(&mut self, row: &UsageRow, cost: Option<u64>, slot: usize, model: usize) {
        self.counts.add(&row.counts);
        self.responses += 1;
        self.unfinalized += u64::from(!row.finalized);
        match cost {
            Some(nanos) => self.cost_nanos = self.cost_nanos.saturating_add(nanos),
            None => self.unpriced += 1,
        }
        self.first_ms = self.first_ms.min(row.at_ms);
        self.last_ms = self.last_ms.max(row.at_ms);
        if !self.slots.contains(&slot) {
            self.slots.push(slot);
        }
        if !self.models.contains(&model) {
            self.models.push(model);
        }
    }

    /// Spec §8.3: no cost when every response is unpriced, a partial one when some are.
    fn cost(&self) -> Option<Cost> {
        (self.unpriced < self.responses).then_some(Cost {
            usd_nanos: self.cost_nanos,
            partial: self.unpriced > 0,
        })
    }
}

impl Tables<'_> {
    /// The groups as rows, newest first, and within a period by account, then model
    /// (spec §10.1).
    fn rows(&self, groups: HashMap<GroupKey, Acc>, now_ms: i64) -> Vec<TokenRow> {
        let slot_ord = ordinals(&self.slots.accounts, |a, b| a.rank().cmp(&b.rank()));
        let model_ord = ordinals(&self.models.names, Ord::cmp);
        let order = |k: &GroupKey| {
            let (newest, tie) = match k.period {
                Period::Day(d) => (
                    i64::from(d.year()) * 10_000 + i64::from(d.month()) * 100 + i64::from(d.day()),
                    "",
                ),
                Period::Month(year, month) => (i64::from(year) * 100 + i64::from(month), ""),
                // Ties keep each session's rows together.
                Period::Session(s) => (
                    self.sessions.list[s].last_ms,
                    self.sessions.list[s].id.as_str(),
                ),
                Period::Block(b) => (self.blocks[b].span.start_ms, ""),
            };
            (
                Reverse(newest),
                tie,
                k.slot.map_or(0, |s| slot_ord[s]),
                k.model.map_or(0, |m| model_ord[m]),
            )
        };
        let mut groups: Vec<(GroupKey, Acc)> = groups.into_iter().collect();
        groups.sort_by(|(a, _), (b, _)| order(a).cmp(&order(b)));
        groups
            .into_iter()
            .map(|(k, acc)| self.row(k, acc, &slot_ord, now_ms))
            .collect()
    }

    fn row(&self, k: GroupKey, mut acc: Acc, slot_ord: &[usize], now_ms: i64) -> TokenRow {
        let cost = acc.cost();
        let period = match k.period {
            Period::Day(date) => PeriodKey::Day { date },
            Period::Month(year, month) => PeriodKey::Month { year, month },
            Period::Session(s) => PeriodKey::Session {
                session_id: self.sessions.list[s].id.clone(),
                project: self.sessions.list[s].project.clone(),
                first_ms: acc.first_ms,
                last_ms: acc.last_ms,
            },
            Period::Block(b) => {
                let block = &self.blocks[b];
                PeriodKey::Block {
                    start_ms: block.span.start_ms,
                    end_ms: block.span.end_ms,
                    observed: block.span.observed,
                    active: project(
                        &block.span,
                        block.first_ms,
                        now_ms,
                        acc.counts.total(),
                        cost.map(|c| c.usd_nanos),
                    )
                    .map(|p| active_block(&p, cost)),
                }
            }
        };
        let account = k.slot.map(|s| &self.slots.accounts[s]);
        let model = k.model.map(|m| self.models.names[m].as_str());
        acc.slots.sort_by_key(|&s| slot_ord[s]);
        let mut models: Vec<String> = acc
            .models
            .iter()
            .map(|&m| self.models.names[m].clone())
            .collect();
        models.sort();
        TokenRow {
            key: RowKey::new(&period, account, model),
            period,
            account: account.cloned(),
            model: model.map(str::to_owned),
            accounts: acc
                .slots
                .iter()
                .map(|&s| self.slots.accounts[s].clone())
                .collect(),
            models,
            counts: acc.counts,
            responses: acc.responses,
            unfinalized: acc.unfinalized,
            cost,
        }
    }
}

/// Each item's place in the order `cmp` gives.
fn ordinals<T>(items: &[T], cmp: impl Fn(&T, &T) -> std::cmp::Ordering) -> Vec<usize> {
    let mut by: Vec<usize> = (0..items.len()).collect();
    by.sort_by(|&a, &b| cmp(&items[a], &items[b]));
    let mut ord = vec![0; items.len()];
    for (o, i) in by.into_iter().enumerate() {
        ord[i] = o;
    }
    ord
}

/// The active block's figures (spec §9), its costs partial as the row's is.
fn active_block(p: &Projection, cost: Option<Cost>) -> ActiveBlock {
    let partial = cost.is_some_and(|c| c.partial);
    let priced = |nanos: Option<u64>| nanos.map(|usd_nanos| Cost { usd_nanos, partial });
    ActiveBlock {
        elapsed_s: p.elapsed_s,
        remaining_s: p.remaining_s,
        tokens_per_min: p.tokens_per_min,
        cost_per_hour: priced(p.cost_nanos_per_hour),
        projected_tokens: p.projected_tokens,
        projected_cost: priced(p.projected_cost_nanos),
    }
}
