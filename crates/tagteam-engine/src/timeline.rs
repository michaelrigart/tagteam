//! §12.3's timeline: History's usage per column and who was in use, bucketed here so the UI
//! thread only draws (T4-3).

use std::collections::HashMap;

use serde_json::Value;
use tagteam_core::usage::is_relevant;
use tagteam_core::{AccountId, ProviderId, Sample, Window};
use tagteam_provider::Provider;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::event::{AUTO_LINE_KINDS, EventKind, EventSource};
use crate::settings::Settings;
use crate::store::{AccountRow, EventRow, Store, StoreError};

/// The all-accounts chart's label (§7.3).
pub const IN_USE: &str = "in use";

/// Whose timeline: every account's, or one account's (§7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    Account(AccountId),
}

/// `columns` columns from `since_ms` to `now_ms` (T4-4): `now_ms` is in the last column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub since_ms: i64,
    pub now_ms: i64,
    pub columns: usize,
}

impl Grid {
    fn span(&self) -> i128 {
        (i128::from(self.now_ms) - i128::from(self.since_ms)).max(1)
    }

    /// The column `at_ms` falls in; `None` before `since_ms`, after `now_ms` or with no columns.
    pub fn column_of(&self, at_ms: i64) -> Option<usize> {
        if self.columns == 0 || at_ms < self.since_ms || at_ms > self.now_ms {
            return None;
        }
        let at = i128::from(at_ms) - i128::from(self.since_ms);
        let column = at * self.columns as i128 / self.span();
        Some((column as usize).min(self.columns - 1))
    }

    /// Where a row stamped at `at_ms` is placed: one less than a second after `now_ms` is
    /// placed at `now_ms`, in the last column. The view's `now_ms` is a whole second while the
    /// store stamps rows in milliseconds, so a change later in the current second would
    /// otherwise fall in no column until the next read. A row from the next second on is left
    /// where it is, after `now_ms`, in no column.
    pub fn placed(&self, at_ms: i64) -> i64 {
        if at_ms > self.now_ms && at_ms < self.now_ms.saturating_add(1000) {
            self.now_ms
        } else {
            at_ms
        }
    }

    /// Where `column` ends, exclusive, and the next one starts; `now_ms` for the last. While
    /// the span is at least the column count, as a day or more over a screen's columns always
    /// is, `column_of(end_ms(c)) == c + 1`.
    pub fn end_ms(&self, column: usize) -> i64 {
        if column + 1 >= self.columns {
            return self.now_ms;
        }
        let n = self.columns as i128;
        let end = ((column as i128 + 1) * self.span() + n - 1) / n;
        (i128::from(self.since_ms) + end) as i64
    }
}

/// An account as the timeline names it, resolved at read time (T4-28).
#[derive(Debug, Clone, PartialEq)]
pub struct AccountRef {
    pub id: AccountId,
    /// `None` once removed.
    pub position: Option<u32>,
    pub alias: Option<String>,
    /// The email, else the label.
    pub email: Option<String>,
    pub removed: bool,
}

impl AccountRef {
    fn of(row: &AccountRow) -> Self {
        AccountRef {
            id: row.id.clone(),
            position: Some(row.position),
            alias: row.alias.clone(),
            email: Some(row.display_email().to_owned()),
            removed: false,
        }
    }

    /// A removed account, named by the label its `remove` event stored (T4-27).
    fn removed(id: &AccountId, label: Option<String>) -> Self {
        AccountRef {
            id: id.clone(),
            position: None,
            alias: None,
            email: label,
            removed: true,
        }
    }
}

/// `id` as it reads now (T4-28): its row when it is the provider's, else removed and named by
/// its `remove` label. `row` is the caller's lookup of the current account.
fn name_of(
    store: &Store,
    provider: &ProviderId,
    id: &AccountId,
    row: Option<&AccountRow>,
) -> Result<AccountRef, StoreError> {
    match row {
        Some(row) => Ok(AccountRef::of(row)),
        None => Ok(AccountRef::removed(id, store.removed_label(provider, id)?)),
    }
}

/// One stored event as History shows it (§12.3), its accounts resolved at read time.
#[derive(Debug, Clone, PartialEq)]
pub struct EventView {
    /// The stored time; its column is `grid.column_of(grid.placed(at_ms))`.
    pub at_ms: i64,
    pub kind: EventKind,
    pub source: EventSource,
    pub trigger: Option<String>,
    pub from: Option<AccountRef>,
    pub to: Option<AccountRef>,
    pub detail: Option<Value>,
    /// For `switch` and `switch-recovered`: the binding usage just before and just after (below).
    pub before_pct: Option<f64>,
    pub after_pct: Option<f64>,
}

impl EventView {
    /// `e`, with its accounts named by `name` and no usage beside it.
    fn of(
        e: EventRow,
        mut name: impl FnMut(&AccountId) -> Result<AccountRef, StoreError>,
    ) -> Result<Self, StoreError> {
        Ok(EventView {
            at_ms: e.at,
            kind: e.kind,
            source: e.source,
            trigger: e.trigger,
            from: e.from_id.as_ref().map(&mut name).transpose()?,
            to: e.to_id.as_ref().map(&mut name).transpose()?,
            detail: e.detail,
            before_pct: None,
            after_pct: None,
        })
    }
}

/// One chart: each column's peak `pct`, `None` where nothing was read.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub label: String,
    pub peaks: Vec<Option<f64>>,
}

/// One account's row: each column's binding usage, and whether it was in use there.
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    pub account: AccountRef,
    pub peaks: Vec<Option<f64>>,
    pub in_use: Vec<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineView {
    pub since_s: i64,
    pub now_s: i64,
    pub threshold: f64,
    pub columns: usize,
    /// All: one series, the in-use account's binding usage. Account: one per relevant window.
    pub charts: Vec<Series>,
    /// All: one per account in use in some column. Account: that account's.
    pub lanes: Vec<Lane>,
    /// The range's events, oldest first. All: every one. Account: those naming it, and every
    /// pause and resume.
    pub events: Vec<EventView>,
    /// The latest pause or resume before the range is a pause: the range starts paused.
    pub paused_at_start: bool,
}

impl TimelineView {
    /// The grid the view was bucketed on (T4-4).
    pub fn grid(&self) -> Grid {
        Grid {
            since_ms: self.since_s * 1000,
            now_ms: self.now_s * 1000,
            columns: self.columns,
        }
    }
}

/// Who was in use from when to when (T4-6): each row's account from its `at` (as `Grid::placed`
/// places it) to the next row's, the last to `now_ms`, ends exclusive. A row with no account is
/// no one's.
fn spans<'r>(rows: &'r [(i64, Option<String>)], grid: &Grid) -> Vec<(&'r str, i64, i64)> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, (at, id))| {
            let end = rows
                .get(i + 1)
                .map_or(grid.now_ms.saturating_add(1), |(next, _)| {
                    grid.placed(*next)
                });
            id.as_deref().map(|id| (id, grid.placed(*at), end))
        })
        .collect()
}

/// The columns `[start_ms, end_ms)` overlaps.
fn columns_of(grid: &Grid, start_ms: i64, end_ms: i64) -> Option<std::ops::RangeInclusive<usize>> {
    if end_ms <= start_ms {
        return None;
    }
    let first = grid.column_of(start_ms.max(grid.since_ms))?;
    let last = grid.column_of((end_ms - 1).min(grid.now_ms))?;
    Some(first..=last)
}

/// Raises column `column` of `peaks` to `pct`.
fn raise(peaks: &mut [Option<f64>], column: usize, pct: f64) {
    let peak = &mut peaks[column];
    *peak = Some(peak.map_or(pct, |p| p.max(pct)));
}

/// Each column's peak of `samples`.
fn peaks_of<'s>(grid: &Grid, samples: impl IntoIterator<Item = &'s Sample>) -> Vec<Option<f64>> {
    let mut peaks = vec![None; grid.columns];
    for s in samples {
        if let Some(c) = grid.column_of(s.fetched_at * 1000) {
            raise(&mut peaks, c, s.pct);
        }
    }
    peaks
}

impl Engine {
    /// §12.3, reading only, never creating the store: `scope`'s charts, lanes and events over
    /// `columns` columns from `since_s` to now (T4-4 to T4-7). Never fetches.
    pub fn timeline(
        &self,
        provider: &ProviderId,
        scope: Scope,
        since_s: i64,
        columns: usize,
    ) -> Result<TimelineView, EngineError> {
        let p = self.provider(provider)?;
        let (settings, _) = Settings::load(&self.env, provider);
        let now_s = self.now_ms().div_euclid(1000);
        let mut view = TimelineView {
            since_s,
            now_s,
            threshold: settings.threshold,
            columns,
            charts: Vec::new(),
            lanes: Vec::new(),
            events: Vec::new(),
            paused_at_start: false,
        };
        let grid = view.grid();
        let missing = |id: &AccountId| EngineError::NoSuchAccount(id.to_string());
        let Some(store) = self.existing_store()? else {
            return match scope {
                Scope::All => {
                    view.charts = vec![Series {
                        label: IN_USE.to_owned(),
                        peaks: vec![None; columns],
                    }];
                    Ok(view)
                }
                Scope::Account(id) => Err(missing(&id)),
            };
        };
        let current = store.accounts(provider)?;
        let by_id: HashMap<&str, &AccountRow> =
            current.iter().map(|r| (r.id.as_str(), r)).collect();
        let mut removed: HashMap<AccountId, AccountRef> = HashMap::new();
        let mut name = |id: &AccountId| -> Result<AccountRef, StoreError> {
            if let Some(known) = removed.get(id) {
                return Ok(known.clone());
            }
            let named = name_of(&store, provider, id, by_id.get(id.as_str()).copied())?;
            if named.removed {
                removed.insert(id.clone(), named.clone());
            }
            Ok(named)
        };
        let mut readings = Readings {
            p: p.as_ref(),
            store: &store,
            models: &settings.models,
            since_s,
            now_ms: grid.now_ms,
            read: HashMap::new(),
            before_range: HashMap::new(),
        };
        let history = store.active_history_since(provider, grid.since_ms)?;
        let spans = spans(&history, &grid);
        let in_use = |id: &str| {
            let mut flags = vec![false; columns];
            for (_, start, end) in spans.iter().filter(|(who, ..)| *who == id) {
                if let Some(cols) = columns_of(&grid, *start, *end) {
                    flags[cols].fill(true);
                }
            }
            flags
        };
        match &scope {
            Scope::Account(id) => {
                let row = by_id.get(id.as_str()).ok_or_else(|| missing(id))?;
                let windows = readings.of(&row.id)?;
                view.lanes = vec![Lane {
                    account: AccountRef::of(row),
                    peaks: peaks_of(&grid, windows.iter().flat_map(|(_, s)| s)),
                    in_use: in_use(row.id.as_str()),
                }];
                view.charts = windows
                    .iter()
                    .map(|(w, samples)| Series {
                        label: w.label.clone(),
                        peaks: peaks_of(&grid, samples),
                    })
                    .collect();
            }
            Scope::All => {
                // Every account in use in some column, in the order of its first use there.
                let mut used: Vec<&str> = Vec::new();
                for (id, start, end) in &spans {
                    if !used.contains(id) && columns_of(&grid, *start, *end).is_some() {
                        used.push(id);
                    }
                }
                let mut accounts: Vec<AccountRef> = current
                    .iter()
                    .filter(|r| used.contains(&r.id.as_str()))
                    .map(AccountRef::of)
                    .collect();
                for id in used.iter().filter(|id| !by_id.contains_key(*id)) {
                    accounts.push(name(&AccountId::from_string(*id))?);
                }
                let mut chart = vec![None; columns];
                for account in accounts {
                    let id = account.id.as_str();
                    let windows = readings.of(&account.id)?;
                    let samples: Vec<&Sample> = windows.iter().flat_map(|(_, s)| s).collect();
                    // The account's spans follow one another, so one search finds a sample's.
                    let own: Vec<(i64, i64)> = spans
                        .iter()
                        .filter(|(who, ..)| *who == id)
                        .map(|(_, start, end)| (*start, *end))
                        .collect();
                    for s in &samples {
                        let at = s.fetched_at * 1000;
                        let after = own.partition_point(|(start, _)| *start <= at);
                        let was_in_use = after > 0 && at < own[after - 1].1;
                        if let (true, Some(c)) = (was_in_use, grid.column_of(at)) {
                            raise(&mut chart, c, s.pct);
                        }
                    }
                    view.lanes.push(Lane {
                        peaks: peaks_of(&grid, samples),
                        in_use: in_use(id),
                        account,
                    });
                }
                view.charts = vec![Series {
                    label: IN_USE.to_owned(),
                    peaks: chart,
                }];
            }
        }
        let pauses = [EventKind::AutoswitchPaused, EventKind::AutoswitchResumed];
        view.paused_at_start = store
            .latest_event(provider, &pauses, None, Some(grid.since_ms))?
            .is_some_and(|e| e.kind == EventKind::AutoswitchPaused);
        let keeps = |e: &EventRow| match &scope {
            Scope::All => true,
            Scope::Account(id) => {
                pauses.contains(&e.kind)
                    || e.from_id.as_ref() == Some(id)
                    || e.to_id.as_ref() == Some(id)
            }
        };
        for e in store.events_since(provider, grid.since_ms)? {
            // As in-use changes are: a row later in the current second is placed at now, one
            // from the next second on is left for the next read.
            if !keeps(&e) || grid.placed(e.at) > grid.now_ms {
                continue;
            }
            let switch = matches!(e.kind, EventKind::Switch | EventKind::SwitchRecovered);
            let (from_id, to_id) = (e.from_id.clone(), e.to_id.clone());
            let mut event = EventView::of(e, &mut name)?;
            if switch {
                if let Some(id) = &from_id {
                    event.before_pct = readings.before(id, event.at_ms)?;
                }
                if let Some(id) = &to_id {
                    event.after_pct = readings.after(id, event.at_ms)?;
                }
            }
            view.events.push(event);
        }
        Ok(view)
    }

    /// §12.2: the newest of the provider's `AUTO_LINE_KINDS` at or after `since_ms`, its
    /// accounts named as they read now; `None` without a store.
    pub fn last_auto_event(
        &self,
        provider: &ProviderId,
        since_ms: i64,
    ) -> Result<Option<EventView>, EngineError> {
        self.provider(provider)?;
        let Some(store) = self.existing_store()? else {
            return Ok(None);
        };
        let Some(e) = store.latest_event(provider, &AUTO_LINE_KINDS, Some(since_ms), None)? else {
            return Ok(None);
        };
        let name = |id: &AccountId| {
            let row = store.account(id)?.filter(|r| &r.provider == provider);
            name_of(&store, provider, id, row.as_ref())
        };
        Ok(Some(EventView::of(e, name)?))
    }
}

/// The accounts' relevant windows and their samples in the range, each account's read once,
/// for its lane and for the usage beside a switch.
struct Readings<'s> {
    p: &'s dyn Provider,
    store: &'s Store,
    models: &'s [String],
    since_s: i64,
    now_ms: i64,
    read: HashMap<AccountId, Vec<(Window, Vec<Sample>)>>,
    /// Each account's reading from before the range, when a switch needed it.
    before_range: HashMap<AccountId, Option<f64>>,
}

impl Readings<'_> {
    fn of(&mut self, id: &AccountId) -> Result<&[(Window, Vec<Sample>)], EngineError> {
        if !self.read.contains_key(id) {
            let windows = relevant_samples(self.p, self.store, id, self.models, self.since_s)?;
            self.read.insert(id.clone(), windows);
        }
        Ok(&self.read[id])
    }

    /// The highest relevant `pct` of `id`'s latest reading at or before `at_ms`: the samples
    /// sharing one `fetched_at`. Older than the range, it is read one row per relevant window.
    fn before(&mut self, id: &AccountId, at_ms: i64) -> Result<Option<f64>, EngineError> {
        self.of(id)?;
        let windows = &self.read[id];
        let in_range = peak_of_reading(
            windows
                .iter()
                .flat_map(|(_, s)| s)
                .filter(|s| s.fetched_at * 1000 <= at_ms)
                .map(|s| (s.fetched_at, s.pct)),
            |at| at,
        );
        if in_range.is_some() {
            return Ok(in_range);
        }
        if let Some(known) = self.before_range.get(id) {
            return Ok(*known);
        }
        let mut latest = Vec::with_capacity(windows.len());
        for (w, _) in windows {
            latest.extend(
                self.store
                    .usage_sample_before(id, &w.key, self.since_s - 1)?,
            );
        }
        let peak = peak_of_reading(latest, |at| at);
        self.before_range.insert(id.clone(), peak);
        Ok(peak)
    }

    /// The highest relevant `pct` of `id`'s earliest reading at or after `at_ms`, up to now.
    fn after(&mut self, id: &AccountId, at_ms: i64) -> Result<Option<f64>, EngineError> {
        let now_ms = self.now_ms;
        let samples = self.of(id)?.iter().flat_map(|(_, s)| s);
        Ok(peak_of_reading(
            samples
                .filter(|s| (at_ms..=now_ms).contains(&(s.fetched_at * 1000)))
                .map(|s| (s.fetched_at, s.pct)),
            |at| -at,
        ))
    }
}

/// One reading's peak: the highest `pct` of the `(fetched_at, pct)` pairs sharing the
/// `fetched_at` that `order` ranks first (`|at| at` the latest, `|at| -at` the earliest).
fn peak_of_reading(
    pairs: impl IntoIterator<Item = (i64, f64)>,
    order: impl Fn(i64) -> i64,
) -> Option<f64> {
    pairs
        .into_iter()
        .fold(None, |best: Option<(i64, f64)>, (at, pct)| match best {
            Some((k, peak)) if k > order(at) => Some((k, peak)),
            Some((k, peak)) if k == order(at) => Some((k, peak.max(pct))),
            _ => Some((order(at), pct)),
        })
        .map(|(_, peak)| peak)
}

/// `id`'s relevant windows under `models` (T4-5, as `Engine::history` finds them), each with
/// its samples since `since_s`: the last good reading's windows in order, then the other keys
/// its samples name, as the provider describes them (a key it does not know is skipped).
fn relevant_samples(
    p: &dyn Provider,
    store: &Store,
    id: &AccountId,
    models: &[String],
    since_s: i64,
) -> Result<Vec<(Window, Vec<Sample>)>, EngineError> {
    let mut windows = match store.usage_state(id)? {
        Some(state) if p.capabilities().usage && state.fetched_at.is_some() => {
            state.last_good.unwrap_or_default()
        }
        _ => Vec::new(),
    };
    for key in store.usage_window_keys(id)? {
        if windows.iter().any(|w| w.key == key) {
            continue;
        }
        windows.extend(p.describe_window(&key));
    }
    windows
        .into_iter()
        .filter(|w| is_relevant(w, models))
        .map(|w| {
            let samples = store
                .usage_samples(id, Some(&w.key), since_s)?
                .into_iter()
                .map(|(_, s)| s)
                .collect();
            Ok((w, samples))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_puts_each_instant_in_its_column() {
        let g = Grid {
            since_ms: 0,
            now_ms: 7000,
            columns: 7,
        };
        for (t, c) in [
            (-1, None),
            (0, Some(0)),
            (999, Some(0)),
            (1000, Some(1)),
            (6999, Some(6)),
            (7000, Some(6)),
            (7001, None),
        ] {
            assert_eq!(g.column_of(t), c, "{t}");
        }
        assert_eq!((g.end_ms(0), g.end_ms(6)), (1000, 7000));
        for (t, placed) in [
            (6999, 6999),
            (7000, 7000),
            (7300, 7000),
            (7999, 7000),
            (8000, 8000),
        ] {
            assert_eq!(g.placed(t), placed, "{t}");
        }

        let g = Grid {
            since_ms: 0,
            now_ms: 10,
            columns: 3,
        };
        for (t, c) in [(3, 0), (4, 1), (6, 1), (7, 2)] {
            assert_eq!(g.column_of(t), Some(c), "{t}");
        }
        assert_eq!((g.end_ms(0), g.end_ms(1), g.end_ms(2)), (4, 7, 10));

        let g = Grid { columns: 0, ..g };
        for t in [-1, 0, 5, 10, 11] {
            assert_eq!(g.column_of(t), None, "{t}");
        }

        // Each column starts where the one before it ends, over the grids History draws.
        let now_ms = 1_790_000_000_000;
        for days in [1, 7, 30] {
            for columns in [70, 110, 200] {
                let g = Grid {
                    since_ms: now_ms - days * 86_400_000,
                    now_ms,
                    columns,
                };
                for c in 0..columns - 1 {
                    let end = g.end_ms(c);
                    assert_eq!(g.column_of(end), Some(c + 1), "{days}d {columns} {c}");
                    assert_eq!(g.column_of(end - 1), Some(c), "{days}d {columns} {c}");
                }
            }
        }
    }
}
