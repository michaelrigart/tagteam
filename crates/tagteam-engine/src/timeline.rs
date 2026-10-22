//! §12.3's timeline: History's usage per column and who was in use, bucketed here so the UI
//! thread only draws (T4-3).

use std::collections::HashMap;

use tagteam_core::usage::is_relevant;
use tagteam_core::{AccountId, ProviderId, Sample, Window};
use tagteam_provider::Provider;

use crate::engine::Engine;
use crate::error::EngineError;
use crate::settings::Settings;
use crate::store::{AccountRow, Store};

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

    /// Where `column` ends, exclusive, and the next one starts; `now_ms` for the last.
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
            email: row.email.clone().or_else(|| Some(row.label.clone())),
            removed: false,
        }
    }

    fn removed(id: &str) -> Self {
        AccountRef {
            id: AccountId::from_string(id),
            position: None,
            alias: None,
            email: None,
            removed: true,
        }
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

/// Who was in use from when to when (T4-6): each row's account from its `at` to the next
/// row's, the last to `now_ms`, ends exclusive. A row with no account is no one's.
fn spans(rows: &[(i64, Option<String>)], now_ms: i64) -> Vec<(&str, i64, i64)> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, (at, id))| {
            let end = rows
                .get(i + 1)
                .map_or(now_ms.saturating_add(1), |(next, _)| *next);
            id.as_deref().map(|id| (id, *at, end))
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
    /// §12.3, reading only, never creating the store: `scope`'s charts and lanes over
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
        let history = store.active_history_since(provider, grid.since_ms)?;
        let spans = spans(&history, grid.now_ms);
        let in_use = |id: &str| {
            let mut flags = vec![false; columns];
            for (_, start, end) in spans.iter().filter(|(who, ..)| *who == id) {
                if let Some(cols) = columns_of(&grid, *start, *end) {
                    flags[cols].fill(true);
                }
            }
            flags
        };
        match scope {
            Scope::Account(id) => {
                let row = current
                    .iter()
                    .find(|r| r.id == id)
                    .ok_or_else(|| missing(&id))?;
                let windows =
                    relevant_samples(p.as_ref(), &store, &row.id, &settings.models, since_s)?;
                view.lanes = vec![Lane {
                    account: AccountRef::of(row),
                    peaks: peaks_of(&grid, windows.iter().flat_map(|(_, s)| s)),
                    in_use: in_use(row.id.as_str()),
                }];
                view.charts = windows
                    .into_iter()
                    .map(|(w, samples)| Series {
                        label: w.label,
                        peaks: peaks_of(&grid, &samples),
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
                let by_id: HashMap<&str, &AccountRow> =
                    current.iter().map(|r| (r.id.as_str(), r)).collect();
                let accounts = current
                    .iter()
                    .filter(|r| used.contains(&r.id.as_str()))
                    .map(AccountRef::of)
                    .chain(
                        used.iter()
                            .filter(|id| !by_id.contains_key(*id))
                            .map(|id| AccountRef::removed(id)),
                    );
                let mut chart = vec![None; columns];
                for account in accounts {
                    let id = account.id.as_str();
                    let windows = relevant_samples(
                        p.as_ref(),
                        &store,
                        &account.id,
                        &settings.models,
                        since_s,
                    )?;
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
        Ok(view)
    }
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
    }
}
