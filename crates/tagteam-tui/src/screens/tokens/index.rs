//! The token index on the Tokens screen (spec §5.3 "Indexing", §6.4): the TUI's own catch-up on
//! the index lane, the index's state for the header, and when the report on screen is read
//! again. A daemon that keeps the index current (live mode) takes the timer's place, and its
//! `token-index` events say when the index changed.

use serde_json::Value;
use tagteam_core::AccountId;
use tagteam_engine::tokens::{IndexProgress, IndexState, PeriodKey, RefreshOutcome, TokenReport};

use super::requery;
use crate::daemon::{self, words};
use crate::msg::Cmd;
use crate::source::SourceError;
use crate::state::{Screen, State};

/// Spec §6.4: in timer mode the index is refreshed this often while Tokens is open, counted
/// from the last refresh's return...
pub const REFRESH_MS: i64 = 60_000;
/// ... the report on screen is read again this often, because the active block, a rolling
/// range and the day's or month's boundary depend on the clock ...
pub const REQUERY_MS: i64 = 60_000;
/// ... and while a refresh runs, the index's state is checked this often, so the table fills
/// in as its batches commit.
pub const CHECK_MS: i64 = 5_000;
/// What a failed check of the index's state is shown as, before the engine's words.
const CHECK_FAILED: &str = "the token index's state was not read: ";
/// `r` while the TUI's own refresh runs.
pub const ALREADY: &str = "the token index is already being refreshed";
/// Spec §6.4 in live mode: the daemon's `token-index` events read the report again at most this
/// often, the 5 s of a refresh's checks. During a cold catch-up they come about once a second.
pub const LIVE_REQUERY_MS: i64 = 5_000;

/// An account as the report's rows name it (`RowAccount::Account`): its ID, position, label
/// and alias.
type AccountKey = (AccountId, u32, String, Option<String>);

/// The index's state as the screen knows it, and when it acts on it.
pub struct IndexWatch {
    /// The index's state as last read: with the last report, or by the last check.
    state: Option<IndexState>,
    /// The TUI's own refresh, while it runs: how far it has come.
    refreshing: Option<IndexProgress>,
    /// The last refresh returned `Busy`: another process is catching up (spec §5.3).
    busy: bool,
    /// The index's `refreshed_at_ms` as the first check after `Busy` read it, once one has: a
    /// later check that sees it move on finds the other process done, and ends `busy`. Not the
    /// last report's value, which may be old, or none.
    busy_after: Option<Option<i64>>,
    /// The first refresh has been asked for (B5).
    started: bool,
    /// The last refresh's failure, in words.
    error: Option<String>,
    next_refresh_ms: i64,
    next_query_ms: i64,
    next_check_ms: i64,
    /// The Tokens provider's accounts as the last `accounts` read listed them (B1).
    accounts: Option<Vec<AccountKey>>,
    /// The end of the active block the report was last read again for.
    ended: Option<i64>,
    /// When a `token-index` event may next read the report again (`LIVE_REQUERY_MS`).
    live_next_ms: i64,
    /// One came sooner: the report is read again at `live_next_ms`.
    live_due: bool,
}

impl Default for IndexWatch {
    /// Nothing read yet. The index is refreshed when the screen is first open; the report is
    /// read again 60 s after one arrives.
    fn default() -> Self {
        IndexWatch {
            state: None,
            refreshing: None,
            busy: false,
            busy_after: None,
            started: false,
            error: None,
            next_refresh_ms: 0,
            next_query_ms: i64::MAX,
            next_check_ms: 0,
            accounts: None,
            ended: None,
            live_next_ms: 0,
            live_due: false,
        }
    }
}

impl IndexWatch {
    /// The index's state as last read.
    pub fn state(&self) -> Option<&IndexState> {
        self.state.as_ref()
    }

    /// The TUI's own refresh's progress while it runs.
    pub fn refreshing(&self) -> Option<&IndexProgress> {
        self.refreshing.as_ref()
    }

    /// Whether the last refresh found another process catching up.
    pub fn busy(&self) -> bool {
        self.busy
    }

    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// A refresh of the index on the index lane (spec §6.4).
fn refresh(state: &mut State) -> Vec<Cmd> {
    let watch = &mut state.tokens.watch;
    watch.started = true;
    watch.refreshing = Some(IndexProgress::default());
    vec![Cmd::RefreshIndex {
        provider: state.tokens.provider.clone(),
    }]
}

/// B5: the first refresh, asked for once the first frame is drawn, so the catch-up never
/// delays it (spec §1.1 criterion 2).
pub(crate) fn first_refresh(state: &mut State) -> Vec<Cmd> {
    if state.tokens.watch.started || state.quitting.is_some() {
        return vec![];
    }
    refresh(state)
}

/// `r` on Tokens (spec §7.1): a refresh at once, on the index lane, so no action holds it up.
pub(crate) fn refresh_now(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.tokens.watch.refreshing.is_some() {
        state.toast(ALREADY, now_ms);
        return vec![];
    }
    refresh(state)
}

/// The index's state from the report lane, under the next generation.
fn check(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    state.gens.index += 1;
    state.tokens.watch.next_check_ms = now_ms + CHECK_MS;
    vec![Cmd::LoadIndexState {
        r#gen: state.gens.index,
        provider: state.tokens.provider.clone(),
    }]
}

/// The refresh's progress, for the header (spec §5.3).
pub(crate) fn on_progress(state: &mut State, progress: IndexProgress) {
    let watch = &mut state.tokens.watch;
    if watch.refreshing.is_some() {
        watch.refreshing = Some(progress);
        state.dirty |= state.screen == Screen::Tokens;
    }
}

/// The refresh returned: the next is due in `REFRESH_MS`, and on Tokens the index's state is
/// checked at once (spec §6.4).
pub(crate) fn on_refreshed(
    state: &mut State,
    result: Result<RefreshOutcome, SourceError>,
    now_ms: i64,
) -> Vec<Cmd> {
    let keeps = daemon::keeps_index(state);
    let watch = &mut state.tokens.watch;
    watch.refreshing = None;
    watch.next_refresh_ms = now_ms + REFRESH_MS;
    match result {
        Ok(RefreshOutcome::Done(_)) => {
            watch.busy = false;
            watch.error = None;
        }
        // With a daemon that keeps the index, another process catching up is the usual state
        // of things, and its events say when the index changes.
        Ok(RefreshOutcome::Busy) => {
            watch.busy = !keeps;
            watch.busy_after = None;
            watch.error = None;
        }
        Err(e) => {
            watch.busy = false;
            watch.error = Some(format!("the token index was not refreshed: {}", e.message));
        }
    }
    if state.screen != Screen::Tokens || state.quitting.is_some() {
        return vec![];
    }
    state.dirty = true;
    check(state, now_ms)
}

/// The index's state from a check. One of an older generation is dropped. When the index
/// changed after the report on screen was read, the report is read again (spec §6.4).
pub(crate) fn on_index_state(
    state: &mut State,
    r#gen: u64,
    result: Result<IndexState, SourceError>,
    now_ms: i64,
) -> Vec<Cmd> {
    if r#gen != state.gens.index {
        return vec![];
    }
    state.dirty |= state.screen == Screen::Tokens;
    let index = match result {
        Ok(index) => index,
        Err(e) => {
            state.tokens.error = Some(format!("{CHECK_FAILED}{}", e.message));
            return vec![];
        }
    };
    if state
        .tokens
        .error
        .as_deref()
        .is_some_and(|e| e.starts_with(CHECK_FAILED))
    {
        state.tokens.error = None;
    }
    let shown = state.tokens.report().map(|r| r.index.changed_at_ms);
    let watch = &mut state.tokens.watch;
    if watch.busy {
        match watch.busy_after {
            None => watch.busy_after = Some(index.refreshed_at_ms),
            Some(after) if index.refreshed_at_ms > after => watch.busy = false,
            Some(_) => {}
        }
    }
    watch.state = Some(index);
    let moved = shown.is_some_and(|shown| index.changed_at_ms > shown);
    if moved && state.screen == Screen::Tokens && state.quitting.is_none() {
        return requery(state, now_ms);
    }
    vec![]
}

/// A query goes out (it was asked for, or Tokens opened): the 60 s re-query counts from now, and
/// an active block that has already ended is covered by it.
pub(crate) fn on_load(state: &mut State, now_ms: i64) {
    let ended = state.tokens.report().and_then(active_end);
    let watch = &mut state.tokens.watch;
    watch.next_query_ms = now_ms + REQUERY_MS;
    if let Some(end) = ended.filter(|end| now_ms >= *end) {
        watch.ended = Some(end);
    }
}

/// A report arrived, or failed: the 60 s re-query counts from now, and a report's index state
/// is the latest read.
pub(crate) fn on_report(state: &mut State, now_ms: i64) {
    let index = state.tokens.report().map(|r| r.index);
    let watch = &mut state.tokens.watch;
    watch.next_query_ms = now_ms + REQUERY_MS;
    if index.is_some() {
        watch.state = index;
    }
}

/// Spec §6.4: the daemon keeps the index current (`daemon::keeps_index`). A `Busy` the TUI's
/// last refresh met is no longer news: the daemon's events, not the 5 s checks, say when the
/// index changes.
pub(crate) fn on_daemon_index(state: &mut State) {
    state.tokens.watch.busy = false;
}

/// Spec §6.4 in live mode, §8: the daemon's `token-index` event for `provider`. While Tokens shows
/// that provider's report, a `changedAt` newer than the report's reads it again, at most every
/// `LIVE_REQUERY_MS`: an event that comes sooner waits for then, and the report read then has
/// every change since. `changedAt` is to the second, so a change in the second the report's
/// was in counts as newer.
pub(crate) fn on_token_index(
    state: &mut State,
    provider: &str,
    raw: &Value,
    now_ms: i64,
) -> Vec<Cmd> {
    let ours = provider == state.tokens.provider().as_str();
    if !ours || state.screen != Screen::Tokens || state.quitting.is_some() {
        return vec![];
    }
    let Some(changed_ms) = raw["changedAt"].as_str().and_then(words::iso_ms) else {
        return vec![];
    };
    let shown = state.tokens.report().and_then(|r| r.index.changed_at_ms);
    if shown.is_some_and(|shown| changed_ms + 1_000 <= shown) {
        return vec![];
    }
    let watch = &mut state.tokens.watch;
    if now_ms < watch.live_next_ms {
        watch.live_due = true;
        return vec![];
    }
    watch.live_next_ms = now_ms + LIVE_REQUERY_MS;
    watch.live_due = false;
    requery(state, now_ms)
}

/// The Tokens provider's accounts in `state`'s last `accounts` read.
fn account_keys(state: &State) -> Vec<AccountKey> {
    let Some(snap) = state.accounts.snapshot() else {
        return vec![];
    };
    snap.providers
        .iter()
        .filter(|p| &p.provider == state.tokens.provider())
        .flat_map(|p| &p.accounts)
        .map(|v| {
            let row = &v.row;
            (
                row.id.clone(),
                row.position,
                row.label.clone(),
                row.alias.clone(),
            )
        })
        .collect()
}

/// A new `accounts` read (B1): when it lists the Tokens provider's accounts differently from
/// the last (an alias, a position, an account added or removed), the rows' labels may be out of
/// date, and while Tokens is open the report is read again (spec §6.4).
pub(crate) fn on_accounts(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let keys = account_keys(state);
    let before = state.tokens.watch.accounts.replace(keys);
    let changed =
        before.is_some_and(|before| Some(&before) != state.tokens.watch.accounts.as_ref());
    if changed && state.screen == Screen::Tokens {
        return requery(state, now_ms);
    }
    vec![]
}

/// The end of the active block in `report`, if it has one.
fn active_end(report: &TokenReport) -> Option<i64> {
    report
        .rows
        .iter()
        .filter_map(|r| match &r.period {
            PeriodKey::Block {
                end_ms,
                active: Some(_),
                ..
            } => Some(*end_ms),
            _ => None,
        })
        .min()
}

/// Spec §6.4 on the UI's tick, while Tokens is open: the refresh every 60 s, unless the daemon
/// keeps the index current; the index's state every 5 s while a refresh runs here or elsewhere
/// (`Busy`); the report again every 60 s, once at the active block's end, and when a
/// `token-index` event waited for `LIVE_REQUERY_MS`.
pub(crate) fn on_tick(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.screen != Screen::Tokens || state.quitting.is_some() {
        return vec![];
    }
    let mut cmds = Vec::new();
    let keeps = daemon::keeps_index(state);
    let watch = &state.tokens.watch;
    if watch.refreshing.is_none() && now_ms >= watch.next_refresh_ms && !keeps {
        cmds.extend(refresh(state));
    }
    let watch = &mut state.tokens.watch;
    if watch.live_due && now_ms >= watch.live_next_ms {
        watch.live_due = false;
        watch.live_next_ms = now_ms + LIVE_REQUERY_MS;
        cmds.extend(requery(state, now_ms));
    }
    let watch = &state.tokens.watch;
    if (watch.refreshing.is_some() || watch.busy) && now_ms >= watch.next_check_ms {
        cmds.extend(check(state, now_ms));
    }
    if state.tokens.pending() {
        return cmds;
    }
    let ended = state
        .tokens
        .report()
        .and_then(active_end)
        .filter(|end| now_ms >= *end && state.tokens.watch.ended != Some(*end));
    if let Some(end) = ended {
        state.tokens.watch.ended = Some(end);
        cmds.extend(requery(state, now_ms));
    } else if now_ms >= state.tokens.watch.next_query_ms {
        state.tokens.watch.next_query_ms = now_ms + REQUERY_MS;
        cmds.extend(requery(state, now_ms));
    }
    cmds
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tagteam_daemon::protocol::{Event, Status, StatusJob};
    use tagteam_engine::tokens::{ActiveBlock, IndexReport, TokenPeriod, TokenRow};

    use super::*;
    use crate::live::Live;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::{drawn, update};

    /// The token index's commands among `cmds`; the views' and the collector's are left out.
    fn token_cmds(cmds: Vec<Cmd>) -> Vec<Cmd> {
        cmds.into_iter()
            .filter(|c| {
                matches!(
                    c,
                    Cmd::LoadReport { .. } | Cmd::LoadIndexState { .. } | Cmd::RefreshIndex { .. }
                )
            })
            .collect()
    }

    fn tick(state: &mut State, at_ms: i64) -> Vec<Cmd> {
        token_cmds(update(state, Msg::Tick, at_ms))
    }

    /// Answers the report query in `cmds` with `rows` at `at_ms`, its index changed at
    /// `changed_ms`.
    fn answer(state: &mut State, cmds: &[Cmd], rows: Vec<TokenRow>, changed_ms: i64, at_ms: i64) {
        let [Cmd::LoadReport { r#gen, query }] = cmds else {
            panic!("{cmds:?}");
        };
        let mut report = token_report(query.clone(), rows);
        report.index.changed_at_ms = Some(changed_ms);
        update(
            state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result: Ok(report),
            },
            at_ms,
        );
    }

    fn done() -> Result<RefreshOutcome, SourceError> {
        Ok(RefreshOutcome::Done(IndexReport {
            complete: true,
            ..IndexReport::default()
        }))
    }

    fn index_changed_at(changed_ms: i64) -> IndexState {
        IndexState {
            refreshed_at_ms: Some(NOW_MS),
            changed_at_ms: Some(changed_ms),
            ..IndexState::default()
        }
    }

    /// Tokens, its first frame drawn (so its refresh runs) and its report read, its index
    /// changed at `NOW_MS - 60_000`.
    fn on_tokens() -> State {
        let mut state = fresh(Screen::Tokens);
        let cmds = state.start();
        assert_eq!(drawn(&mut state), [Cmd::RefreshIndex { provider: cc() }]);
        answer(
            &mut state,
            &cmds[1..],
            vec![token_row(
                day(0),
                Some(row_account(1)),
                None,
                counts(1, 2, 3),
                None,
            )],
            NOW_MS - 60_000,
            NOW_MS,
        );
        state
    }

    #[test]
    fn the_first_frame_starts_one_refresh_whatever_the_screen_shows() {
        // B5: after the first frame, never before it, and once.
        let mut state = fresh(Screen::Accounts);
        state.start();
        assert_eq!(drawn(&mut state), [Cmd::RefreshIndex { provider: cc() }]);
        assert!(drawn(&mut state).is_empty());
        assert!(state.tokens.watch().refreshing().is_some());
        let progress = IndexProgress {
            bytes_done: 1_200_000_000,
            bytes_total: 3_300_000_000,
            ..IndexProgress::default()
        };
        update(&mut state, Msg::IndexProgress(progress), NOW_MS);
        assert_eq!(state.tokens.watch().refreshing(), Some(&progress));
        let cmds = update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        assert!(cmds.is_empty(), "off Tokens nothing is checked: {cmds:?}");
        assert!(state.tokens.watch().refreshing().is_none());
        assert!(
            tick(&mut state, NOW_MS + 120_000).is_empty(),
            "nor refreshed"
        );
        let mut quitting = fresh(Screen::Accounts);
        quitting.quitting = Some(0);
        assert!(drawn(&mut quitting).is_empty());
    }

    #[test]
    fn on_tokens_the_index_is_refreshed_every_60_s_from_the_last_return() {
        let mut state = on_tokens();
        assert!(
            tick(&mut state, NOW_MS + 120_000)
                .iter()
                .all(|c| !matches!(c, Cmd::RefreshIndex { .. })),
            "one runs already"
        );
        let cmds = update(
            &mut state,
            Msg::Refreshed { result: done() },
            NOW_MS + 10_000,
        );
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadIndexState { .. }]),
            "checked after each refresh returns: {cmds:?}"
        );
        assert!(!tick(&mut state, NOW_MS + 69_999).contains(&Cmd::RefreshIndex { provider: cc() }));
        assert!(tick(&mut state, NOW_MS + 70_000).contains(&Cmd::RefreshIndex { provider: cc() }));
        // Away from Tokens it waits; back on Tokens a due refresh goes at once.
        update(
            &mut state,
            Msg::Refreshed { result: done() },
            NOW_MS + 71_000,
        );
        update(&mut state, ch('1'), NOW_MS + 71_000);
        assert!(tick(&mut state, NOW_MS + 200_000).is_empty());
        update(&mut state, ch('2'), NOW_MS + 200_000);
        assert!(tick(&mut state, NOW_MS + 200_250).contains(&Cmd::RefreshIndex { provider: cc() }));
    }

    #[test]
    fn while_a_refresh_runs_a_check_every_5_s_reads_the_report_again_only_when_the_index_moved() {
        // Spec §6.4: the table fills in as batches commit, and a check that finds no change
        // reads nothing.
        let mut state = on_tokens();
        let check = tick(&mut state, NOW_MS + 1_000);
        let [Cmd::LoadIndexState { r#gen, .. }] = check.as_slice() else {
            panic!("{check:?}");
        };
        assert!(tick(&mut state, NOW_MS + 5_999).is_empty(), "every 5 s");
        let same = Msg::IndexState {
            r#gen: *r#gen,
            result: Ok(index_changed_at(NOW_MS - 60_000)),
        };
        assert!(update(&mut state, same, NOW_MS + 2_000).is_empty());
        let check = tick(&mut state, NOW_MS + 6_000);
        let [Cmd::LoadIndexState { r#gen, .. }] = check.as_slice() else {
            panic!("{check:?}");
        };
        let moved = Msg::IndexState {
            r#gen: *r#gen,
            result: Ok(index_changed_at(NOW_MS + 5_500)),
        };
        let cmds = update(&mut state, moved, NOW_MS + 6_100);
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadReport { .. }]),
            "{cmds:?}"
        );
        assert_eq!(
            state.tokens.watch().state().and_then(|s| s.changed_at_ms),
            Some(NOW_MS + 5_500)
        );
        // No refresh, no checks.
        update(
            &mut state,
            Msg::Refreshed { result: done() },
            NOW_MS + 7_000,
        );
        let later = tick(&mut state, NOW_MS + 30_000);
        assert!(
            later
                .iter()
                .all(|c| !matches!(c, Cmd::LoadIndexState { .. })),
            "{later:?}"
        );
    }

    #[test]
    fn a_check_of_an_older_generation_is_dropped() {
        let mut state = on_tokens();
        let older = tick(&mut state, NOW_MS + 1_000);
        let [Cmd::LoadIndexState { r#gen, .. }] = older.as_slice() else {
            panic!("{older:?}");
        };
        tick(&mut state, NOW_MS + 6_000);
        let stale = Msg::IndexState {
            r#gen: *r#gen,
            result: Ok(index_changed_at(NOW_MS + 5_500)),
        };
        assert!(update(&mut state, stale, NOW_MS + 6_100).is_empty());
    }

    #[test]
    fn busy_keeps_checking_while_another_process_catches_up() {
        // B10: the table fills in as the other process's batches commit.
        let mut state = on_tokens();
        let busy = Ok(RefreshOutcome::Busy);
        update(&mut state, Msg::Refreshed { result: busy }, NOW_MS + 1_000);
        assert!(state.tokens.watch().busy());
        assert!(
            tick(&mut state, NOW_MS + 7_000)
                .iter()
                .any(|c| matches!(c, Cmd::LoadIndexState { .. }))
        );
        update(
            &mut state,
            Msg::Refreshed { result: done() },
            NOW_MS + 61_000,
        );
        assert!(!state.tokens.watch().busy());
    }

    #[test]
    fn busy_ends_when_a_check_sees_the_other_process_finish() {
        // The other process's refresh finished when the index's `refreshed_at_ms` moved on: the
        // header stops saying it is catching up, and the checks stop, without waiting for
        // our own next refresh.
        let mut state = on_tokens();
        let busy = Ok(RefreshOutcome::Busy);
        update(&mut state, Msg::Refreshed { result: busy }, NOW_MS + 1_000);
        let checked = |state: &mut State, at_ms: i64, index: IndexState| {
            let cmds = tick(state, at_ms);
            let [Cmd::LoadIndexState { r#gen, .. }] = cmds.as_slice() else {
                panic!("{cmds:?}");
            };
            let msg = Msg::IndexState {
                r#gen: *r#gen,
                result: Ok(index),
            };
            update(state, msg, at_ms + 100)
        };
        let unchanged = IndexState {
            refreshed_at_ms: Some(NOW_MS - 60_000),
            changed_at_ms: Some(NOW_MS - 60_000),
            ..IndexState::default()
        };
        assert!(checked(&mut state, NOW_MS + 7_000, unchanged).is_empty());
        assert!(state.tokens.watch().busy(), "it has not finished");
        assert!(
            token_cmds(checked(
                &mut state,
                NOW_MS + 12_000,
                index_changed_at(NOW_MS - 60_000)
            ))
            .is_empty()
        );
        assert!(!state.tokens.watch().busy(), "it finished");
        let later = tick(&mut state, NOW_MS + 20_000);
        assert!(
            later
                .iter()
                .all(|c| !matches!(c, Cmd::LoadIndexState { .. })),
            "nothing left to check: {later:?}"
        );
    }

    /// Answers the check among `cmds` with an index refreshed at `refreshed_ms`, at `at_ms`.
    fn answer_check(state: &mut State, cmds: &[Cmd], refreshed_ms: i64, at_ms: i64) {
        let r#gen = cmds
            .iter()
            .find_map(|c| match c {
                Cmd::LoadIndexState { r#gen, .. } => Some(*r#gen),
                _ => None,
            })
            .expect("a check was asked for");
        let index = IndexState {
            refreshed_at_ms: Some(refreshed_ms),
            changed_at_ms: Some(NOW_MS - 3_600_000),
            ..IndexState::default()
        };
        let msg = Msg::IndexState {
            r#gen,
            result: Ok(index),
        };
        update(state, msg, at_ms);
    }

    #[test]
    fn busy_returned_before_any_read_stays_while_the_other_refresh_runs() {
        // The first refresh can return `Busy` before the first report: the index's last
        // completed refresh, which the first check reads, is not the other process finishing.
        let mut state = fresh(Screen::Tokens);
        state.start();
        assert_eq!(drawn(&mut state), [Cmd::RefreshIndex { provider: cc() }]);
        assert!(state.tokens.watch().state().is_none());
        let busy = Ok(RefreshOutcome::Busy);
        let cmds = update(&mut state, Msg::Refreshed { result: busy }, NOW_MS + 100);
        answer_check(&mut state, &cmds, NOW_MS - 3_600_000, NOW_MS + 200);
        assert!(
            state.tokens.watch().busy(),
            "the first check is the baseline"
        );
        let cmds = tick(&mut state, NOW_MS + 5_300);
        answer_check(&mut state, &cmds, NOW_MS - 3_600_000, NOW_MS + 5_400);
        assert!(state.tokens.watch().busy(), "nothing has advanced");
        let cmds = tick(&mut state, NOW_MS + 10_500);
        answer_check(&mut state, &cmds, NOW_MS + 10_000, NOW_MS + 10_600);
        assert!(!state.tokens.watch().busy(), "it finished");
    }

    #[test]
    fn busy_is_judged_from_the_first_check_after_it_and_not_from_an_older_report() {
        // The report on screen was read up to 60 s before `Busy` came back: the other process
        // may have finished one refresh and started another since. The first check, not that
        // report, is what a later one is compared with.
        let mut state = on_tokens();
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        let busy = Ok(RefreshOutcome::Busy);
        let cmds = update(&mut state, Msg::Refreshed { result: busy }, NOW_MS + 60_000);
        answer_check(&mut state, &cmds, NOW_MS + 30_000, NOW_MS + 60_100);
        assert!(
            state.tokens.watch().busy(),
            "a refresh finished before the check; the next one runs"
        );
        let cmds = tick(&mut state, NOW_MS + 65_200);
        answer_check(&mut state, &cmds, NOW_MS + 30_000, NOW_MS + 65_300);
        assert!(state.tokens.watch().busy(), "still the same refresh");
        let cmds = tick(&mut state, NOW_MS + 70_400);
        answer_check(&mut state, &cmds, NOW_MS + 70_000, NOW_MS + 70_500);
        assert!(!state.tokens.watch().busy(), "it finished");
    }

    #[test]
    fn an_account_list_that_changed_reads_the_report_again() {
        // B1, spec §6.4: an alias, a position, an account added or removed.
        let mut state = on_tokens();
        let read = |state: &mut State, accounts| {
            state.gens.accounts += 1;
            let r#gen = state.gens.accounts;
            token_cmds(update(
                state,
                Msg::Accounts {
                    r#gen,
                    result: Ok(snapshot(accounts)),
                },
                NOW_MS,
            ))
        };
        assert!(read(&mut state, many(2)).is_empty(), "the first read");
        assert!(read(&mut state, many(2)).is_empty(), "the same accounts");
        let mut aliased = many(2);
        aliased[0].row.alias = Some("work".into());
        let again = read(&mut state, aliased.clone());
        assert!(
            matches!(again.as_slice(), [Cmd::LoadReport { .. }]),
            "{again:?}"
        );
        answer(&mut state, &again, vec![], NOW_MS - 60_000, NOW_MS);
        aliased[1].row.position = 3;
        let again = read(&mut state, aliased);
        assert_eq!(again.len(), 1, "a position");
        answer(&mut state, &again, vec![], NOW_MS - 60_000, NOW_MS);
        let again = read(&mut state, many(1));
        assert_eq!(again.len(), 1, "an account removed");
        answer(&mut state, &again, vec![], NOW_MS - 60_000, NOW_MS);
        update(&mut state, ch('1'), NOW_MS);
        assert!(read(&mut state, many(2)).is_empty(), "off Tokens");
        let cmds = update(&mut state, ch('2'), NOW_MS);
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadReport { .. }]),
            "opening Tokens reads it again (B19): {cmds:?}"
        );
    }

    #[test]
    fn the_report_is_read_again_every_60_s() {
        let mut state = on_tokens();
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        let reports = |cmds: &[Cmd]| {
            cmds.iter()
                .filter(|c| matches!(c, Cmd::LoadReport { .. }))
                .count()
        };
        assert_eq!(reports(&tick(&mut state, NOW_MS + 59_999)), 0);
        let cmds = tick(&mut state, NOW_MS + 60_000);
        assert_eq!(reports(&cmds), 1, "{cmds:?}");
        let again: Vec<Cmd> = cmds
            .into_iter()
            .filter(|c| matches!(c, Cmd::LoadReport { .. }))
            .collect();
        answer(&mut state, &again, vec![], NOW_MS - 60_000, NOW_MS + 61_000);
        assert_eq!(
            reports(&tick(&mut state, NOW_MS + 120_999)),
            0,
            "60 s from it"
        );
        assert_eq!(reports(&tick(&mut state, NOW_MS + 121_000)), 1);
    }

    fn block(start_ms: i64, end_ms: i64, active: bool) -> TokenRow {
        let period = PeriodKey::Block {
            start_ms,
            end_ms,
            observed: true,
            active: active.then_some(ActiveBlock {
                elapsed_s: (NOW_MS - start_ms) / 1000,
                remaining_s: (end_ms - NOW_MS) / 1000,
                tokens_per_min: 100.0,
                cost_per_hour: None,
                projected_tokens: 1_000,
                projected_cost: None,
            }),
        };
        token_row(period, Some(row_account(1)), None, counts(1, 2, 3), None)
    }

    #[test]
    fn the_active_blocks_end_reads_the_report_again_once() {
        let mut state = on_tokens();
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        update(&mut state, ch('p'), NOW_MS);
        update(&mut state, ch('p'), NOW_MS);
        let blocks = update(&mut state, ch('p'), NOW_MS);
        let [Cmd::LoadReport { query, .. }] = blocks.as_slice() else {
            panic!("{blocks:?}");
        };
        assert_eq!(query.period, TokenPeriod::Block);
        // It ends before the 60 s re-query is due.
        let end = NOW_MS + 30_000;
        let rows = vec![
            block(end - 5 * 3_600_000, end, true),
            block(end - 10 * 3_600_000, end - 5 * 3_600_000, false),
        ];
        answer(&mut state, &blocks, rows, NOW_MS - 60_000, NOW_MS);
        let reports = |cmds: Vec<Cmd>| {
            cmds.into_iter()
                .filter(|c| matches!(c, Cmd::LoadReport { .. }))
                .collect::<Vec<_>>()
        };
        assert!(reports(tick(&mut state, end - 1)).is_empty());
        let at_end = reports(tick(&mut state, end));
        assert_eq!(at_end.len(), 1, "at its end");
        let rows = vec![block(end - 5 * 3_600_000, end, true)];
        answer(&mut state, &at_end, rows, NOW_MS - 60_000, end);
        assert!(
            reports(tick(&mut state, end + 1_000)).is_empty(),
            "once, though the report read again still says active"
        );
    }

    #[test]
    fn r_refreshes_the_index_at_once_and_says_so_while_one_runs() {
        let mut state = on_tokens();
        assert!(update(&mut state, ch('r'), NOW_MS).is_empty());
        assert_eq!(state.toast.as_ref().map(|t| t.text.as_str()), Some(ALREADY));
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        state.running = Some(crate::host::Action::AddLive { provider: cc() });
        assert_eq!(
            update(&mut state, ch('r'), NOW_MS),
            [Cmd::RefreshIndex { provider: cc() }],
            "on its own lane: an action does not hold it up"
        );
    }

    #[test]
    fn a_failed_refresh_shows_on_tokens_and_the_next_good_one_clears_it() {
        let mut state = on_tokens();
        let failed = Err(SourceError {
            kind: "store".into(),
            message: "the store is locked".into(),
        });
        update(&mut state, Msg::Refreshed { result: failed }, NOW_MS);
        assert_eq!(
            draw(&state, 80, 24)[1],
            " the token index was not refreshed: the store is locked"
        );
        update(&mut state, ch('r'), NOW_MS);
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        assert_eq!(state.tokens.error(), None);
    }

    #[test]
    fn returning_to_tokens_after_a_while_reads_the_report_once() {
        let mut state = on_tokens();
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        update(&mut state, ch('1'), NOW_MS + 1_000);
        let cmds = token_cmds(update(&mut state, ch('2'), NOW_MS + 300_000));
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadReport { .. }]),
            "{cmds:?}"
        );
        let ticked = tick(&mut state, NOW_MS + 300_250);
        assert!(
            ticked.iter().all(|c| !matches!(c, Cmd::LoadReport { .. })),
            "{ticked:?}"
        );
        let rest = {
            let [Cmd::LoadReport { r#gen, query }] = cmds.as_slice() else {
                unreachable!()
            };
            let mut report = token_report(query.clone(), vec![]);
            report.index.changed_at_ms = Some(NOW_MS - 60_000);
            update(
                &mut state,
                Msg::TokenReport {
                    r#gen: *r#gen,
                    result: Ok(report),
                },
                NOW_MS + 300_300,
            )
        };
        assert!(
            token_cmds(rest).is_empty(),
            "a second read after the first returns"
        );
    }

    #[test]
    fn returning_to_tokens_past_an_active_blocks_end_reads_the_report_once() {
        let mut state = on_tokens();
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        update(&mut state, ch('p'), NOW_MS);
        update(&mut state, ch('p'), NOW_MS);
        let blocks = update(&mut state, ch('p'), NOW_MS);
        let end = NOW_MS + 30_000;
        let rows = vec![block(end - 5 * 3_600_000, end, true)];
        answer(&mut state, &blocks, rows, NOW_MS - 60_000, NOW_MS);
        update(&mut state, ch('1'), NOW_MS + 1_000);
        let later = end + 10_000;
        let cmds = token_cmds(update(&mut state, ch('2'), later));
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadReport { .. }]),
            "{cmds:?}"
        );
        let ticked = tick(&mut state, later + 250);
        assert!(
            ticked.iter().all(|c| !matches!(c, Cmd::LoadReport { .. })),
            "{ticked:?}"
        );
        let [Cmd::LoadReport { r#gen, query }] = cmds.as_slice() else {
            unreachable!()
        };
        let mut report = token_report(query.clone(), vec![block(end - 5 * 3_600_000, end, true)]);
        report.index.changed_at_ms = Some(NOW_MS - 60_000);
        let rest = update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result: Ok(report),
            },
            later + 300,
        );
        assert!(token_cmds(rest).is_empty(), "a second read after the first");
        assert!(
            tick(&mut state, later + 500)
                .iter()
                .all(|c| !matches!(c, Cmd::LoadReport { .. }))
        );
    }

    #[test]
    fn a_follow_up_query_asked_on_tokens_is_not_sent_once_tokens_is_left() {
        // B2: a query asked for while one runs follows it, but only while Tokens is open.
        let mut state = on_tokens();
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        let first = tick(&mut state, NOW_MS + 60_000);
        let reports: Vec<Cmd> = first
            .into_iter()
            .filter(|c| matches!(c, Cmd::LoadReport { .. }))
            .collect();
        assert_eq!(reports.len(), 1);
        for accounts in [many(1), many(2)] {
            state.gens.accounts += 1;
            let r#gen = state.gens.accounts;
            let cmds = update(
                &mut state,
                Msg::Accounts {
                    r#gen,
                    result: Ok(snapshot(accounts)),
                },
                NOW_MS + 60_100,
            );
            assert!(token_cmds(cmds).is_empty(), "one is on the lane already");
        }
        assert!(state.tokens.again, "a follow-up is asked for");
        update(&mut state, ch('1'), NOW_MS + 60_300);
        let [Cmd::LoadReport { r#gen, query }] = reports.as_slice() else {
            unreachable!()
        };
        let mut report = token_report(query.clone(), vec![]);
        report.index.changed_at_ms = Some(NOW_MS - 60_000);
        let after = update(
            &mut state,
            Msg::TokenReport {
                r#gen: *r#gen,
                result: Ok(report),
            },
            NOW_MS + 61_000,
        );
        assert!(token_cmds(after).is_empty(), "off Tokens nothing is read");
        assert!(!state.tokens.again);
    }

    #[test]
    fn a_failed_check_says_so_and_the_next_good_one_clears_it() {
        let mut state = on_tokens();
        let check = tick(&mut state, NOW_MS + 1_000);
        let [Cmd::LoadIndexState { r#gen, .. }] = check.as_slice() else {
            panic!("{check:?}");
        };
        let failed = Err(SourceError {
            kind: "store".into(),
            message: "the store is locked".into(),
        });
        update(
            &mut state,
            Msg::IndexState {
                r#gen: *r#gen,
                result: failed,
            },
            NOW_MS + 1_100,
        );
        assert_eq!(
            state.tokens.error(),
            Some("the token index's state was not read: the store is locked")
        );
        let check = tick(&mut state, NOW_MS + 6_000);
        let [Cmd::LoadIndexState { r#gen, .. }] = check.as_slice() else {
            panic!("{check:?}");
        };
        update(
            &mut state,
            Msg::IndexState {
                r#gen: *r#gen,
                result: Ok(index_changed_at(NOW_MS - 60_000)),
            },
            NOW_MS + 6_100,
        );
        assert_eq!(state.tokens.error(), None);
    }

    /// The fixture daemon switching Claude Code's accounts, with the index job (D13), whose last
    /// run failed with `error`, when `error` is given.
    fn with_index_job(error: Option<&str>) -> Status {
        let job = StatusJob {
            name: "token-index".into(),
            last_run_at: None,
            next_run_at: None,
            last_error: error.map(str::to_owned),
        };
        daemon_status(vec![daemon_job("claude-code", "auto", true)], vec![job])
    }

    /// `on_tokens`, its first refresh returned at `NOW_MS`, then live on a daemon whose
    /// `status` is `status`, and the report the link reads again answered: changed a minute
    /// before `NOW_MS`, as before.
    fn live_on_tokens(status: Status) -> State {
        let mut state = on_tokens();
        update(&mut state, Msg::Refreshed { result: done() }, NOW_MS);
        go_live(&mut state, status);
        let again = tick(&mut state, NOW_MS + 250);
        answer(&mut state, &again, vec![], NOW_MS - 60_000, NOW_MS + 250);
        state
    }

    /// The daemon's `token-index` event for `provider`, its index changed at `changed_ms`.
    fn token_index(provider: &str, changed_ms: Option<i64>) -> Msg {
        let changed =
            changed_ms.map(|ms| jiff::Timestamp::from_millisecond(ms).unwrap().to_string());
        let raw = json!({"event": "token-index", "provider": provider, "changedAt": changed});
        Msg::Live(Live::Event(Event::TokenIndex {
            provider: provider.into(),
            raw,
        }))
    }

    fn refreshes(cmds: &[Cmd]) -> bool {
        cmds.contains(&Cmd::RefreshIndex { provider: cc() })
    }

    /// `tick` on a live TUI whose daemon answers each `status` it asks for with the last one,
    /// as a working daemon does (C-R19 drops a link whose `status` does not come).
    fn live_tick(state: &mut State, at_ms: i64) -> Vec<Cmd> {
        let cmds = update(state, Msg::Tick, at_ms);
        for c in &cmds {
            if let (Cmd::FetchStatus { link }, Some(status)) = (c, state.daemon.status()) {
                let result = Ok(status.clone());
                update(
                    state,
                    Msg::DaemonStatus {
                        link: *link,
                        result,
                    },
                    at_ms,
                );
            }
        }
        token_cmds(cmds)
    }

    #[test]
    fn a_daemon_that_keeps_the_index_takes_the_timer_s_place_and_r_still_refreshes() {
        // Spec §6.4: in live mode with the index job, no refresh on a timer; `r` is a person's
        // request. Without the link, the timer is back.
        let mut state = live_on_tokens(with_index_job(None));
        assert!(!refreshes(&live_tick(&mut state, NOW_MS + 60_000)));
        assert!(!refreshes(&live_tick(&mut state, NOW_MS + 180_000)));
        assert!(state.daemon.live(), "the daemon answered throughout");
        assert_eq!(
            update(&mut state, ch('r'), NOW_MS + 181_000),
            [Cmd::RefreshIndex { provider: cc() }]
        );
        update(
            &mut state,
            Msg::Refreshed { result: done() },
            NOW_MS + 182_000,
        );
        update(&mut state, Msg::Live(Live::Down), NOW_MS + 250_000);
        assert!(
            refreshes(&tick(&mut state, NOW_MS + 250_250)),
            "60 s after the last returned"
        );
    }

    #[test]
    fn a_daemon_that_runs_another_provider_only_leaves_the_timer_on() {
        // C-R12: the index job serves the providers the daemon runs (D13); a daemon started
        // with `--provider fake-agent` keeps no Claude Code index, however well its job runs.
        let job = StatusJob {
            name: "token-index".into(),
            last_run_at: None,
            next_run_at: None,
            last_error: None,
        };
        let other = daemon_status(vec![daemon_job("fake-agent", "auto", true)], vec![job]);
        let mut state = live_on_tokens(other);
        assert!(refreshes(&tick(&mut state, NOW_MS + 60_000)));
    }

    #[test]
    fn without_the_index_job_or_with_one_that_fails_here_the_timer_refreshes() {
        // Spec §6.4: a daemon without that job leaves the TUI refreshing as in timer mode; one
        // whose job fails for this provider, or for the store, too.
        for (status, timer) in [
            (
                daemon_status(vec![daemon_job("claude-code", "auto", true)], vec![]),
                true,
            ),
            (
                with_index_job(Some("claude-code: a transcript could not be read")),
                true,
            ),
            (
                with_index_job(Some("store error: database is locked")),
                true,
            ),
            (
                with_index_job(Some("fake-agent: a transcript could not be read")),
                false,
            ),
            (with_index_job(None), false),
        ] {
            let errors = status.jobs.first().and_then(|j| j.last_error.clone());
            let mut state = live_on_tokens(status);
            assert_eq!(
                refreshes(&tick(&mut state, NOW_MS + 60_000)),
                timer,
                "{errors:?}"
            );
        }
    }

    #[test]
    fn a_token_index_event_newer_than_the_report_on_screen_reads_it_again() {
        // Spec §6.4, §8. `changedAt` is to the second: one in the report's own second counts as
        // newer.
        let shown = NOW_MS - 60_000;
        let mut state = live_on_tokens(with_index_job(None));
        let at = NOW_MS + 1_000;
        for (msg, why) in [
            (token_index("claude-code", Some(shown - 1_000)), "older"),
            (token_index("claude-code", None), "never changed"),
            (
                token_index("fake-agent", Some(NOW_MS)),
                "another provider's",
            ),
        ] {
            assert!(token_cmds(update(&mut state, msg, at)).is_empty(), "{why}");
        }
        let again = token_cmds(update(
            &mut state,
            token_index("claude-code", Some(shown)),
            at,
        ));
        assert!(
            matches!(again.as_slice(), [Cmd::LoadReport { .. }]),
            "{again:?}"
        );
        answer(&mut state, &again, vec![], NOW_MS + 500, at);
        // Off Tokens the event reads nothing; opening Tokens reads it again anyway (B19).
        update(&mut state, ch('1'), at + 10_000);
        let later = token_index("claude-code", Some(NOW_MS + 9_000));
        assert!(token_cmds(update(&mut state, later, at + 10_000)).is_empty());
    }

    #[test]
    fn a_catch_up_s_stream_of_events_reads_the_report_every_5_s_and_reads_the_last_change() {
        // During a cold catch-up `changedAt` moves about once a second: the report lane is
        // asked at most every `LIVE_REQUERY_MS`, and the change that came last is read.
        let mut state = live_on_tokens(with_index_job(None));
        let mut read_at = Vec::new();
        let mut changed = NOW_MS;
        let mut serve = |state: &mut State, cmds: Vec<Cmd>, at: i64, changed: i64| {
            if !cmds.is_empty() {
                answer(state, &cmds, vec![], changed, at);
                read_at.push(at - NOW_MS);
            }
        };
        for s in 1..=12 {
            let at = NOW_MS + s * 1_000;
            changed = at;
            let cmds = token_cmds(update(&mut state, token_index("claude-code", Some(at)), at));
            serve(&mut state, cmds, at, changed);
            for quarter in 1..4 {
                let cmds = tick(&mut state, at + quarter * 250);
                serve(&mut state, cmds, at + quarter * 250, changed);
            }
        }
        for quarter in 1..=72 {
            let at = NOW_MS + 12_750 + quarter * 250;
            let cmds = tick(&mut state, at);
            serve(&mut state, cmds, at, changed);
        }
        assert_eq!(read_at, [1_000, 6_000, 11_000, 16_000]);
        assert_eq!(
            state.tokens.report().and_then(|r| r.index.changed_at_ms),
            Some(NOW_MS + 12_000),
            "the last change is on screen"
        );
    }

    #[test]
    fn a_busy_refresh_is_not_news_while_the_daemon_keeps_the_index() {
        // B10's `catching up elsewhere` and its 5 s checks are timer mode's: with a daemon that
        // keeps the index, its events say when the index changes.
        let mut state = on_tokens();
        let busy = || Msg::Refreshed {
            result: Ok(RefreshOutcome::Busy),
        };
        update(&mut state, busy(), NOW_MS);
        assert!(state.tokens.watch().busy());
        go_live(&mut state, with_index_job(None));
        assert!(!state.tokens.watch().busy());
        let shown = draw(&state, 80, 24).join("\n");
        assert!(!shown.contains("catching up elsewhere"), "{shown}");
        let checks = |cmds: Vec<Cmd>| {
            cmds.iter()
                .filter(|c| matches!(c, Cmd::LoadIndexState { .. }))
                .count()
        };
        assert_eq!(checks(tick(&mut state, NOW_MS + 7_000)), 0);
        update(&mut state, ch('r'), NOW_MS + 8_000);
        update(&mut state, busy(), NOW_MS + 8_000);
        assert!(!state.tokens.watch().busy(), "a person's `r` too");
    }
}
