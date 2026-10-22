//! The Tokens screen (spec §5.3): the token report (token report §10) by day, month, session or
//! 5-hour block, grouped and filtered, with drill-down from a period to its sessions and from a
//! session to its models. Every query runs on the report lane (§4.3), never on the UI thread,
//! and the index's catch-up on the index lane (`index`).

mod amounts;
pub mod filter;
mod headline;
pub mod index;
mod models;
mod table;

use std::cell::OnceCell;

use crossterm::event::{KeyCode, KeyEvent};
use jiff::tz::TimeZone;
use tagteam_core::ProviderId;
use tagteam_engine::tokens::{
    AccountSel, RowKey, TokenGrouping, TokenPeriod, TokenQuery, TokenReport, TokenRow,
    default_since_ms,
};

use self::index::IndexWatch;
use self::models::ShortModels;
pub use self::table::render;
use crate::msg::Cmd;
use crate::source::SourceError;
use crate::state::{Screen, State};

const DAY_MS: i64 = 86_400_000;

/// The ranges `[` and `]` cycle through (spec §5.3, B9): 7 days, the default (30 days, or for
/// `Month` the twelve calendar months to this one, token report §11.1), 90 days, or all time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    Week,
    #[default]
    Default,
    Quarter,
    All,
}

impl Range {
    const ALL: [Range; 4] = [Range::Week, Range::Default, Range::Quarter, Range::All];

    fn step(self, by: isize) -> Range {
        let at = Self::ALL.iter().position(|r| *r == self).unwrap_or(1) as isize;
        Self::ALL[(at + by).rem_euclid(Self::ALL.len() as isize) as usize]
    }

    /// Where the range starts at `now_ms` for `period`, in `tz`; `None` for all.
    fn since_ms(self, period: TokenPeriod, now_ms: i64, tz: &TimeZone) -> Option<i64> {
        match self {
            Range::Week => Some(now_ms.saturating_sub(7 * DAY_MS)),
            Range::Default => Some(default_since_ms(&period, now_ms, tz)),
            Range::Quarter => Some(now_ms.saturating_sub(90 * DAY_MS)),
            Range::All => None,
        }
    }

    /// The control bar's words for the range of a `period` report (T5-6).
    pub fn label(self, period: TokenPeriod) -> &'static str {
        match (self, period) {
            (Range::Week, _) => "7 days",
            (Range::Default, TokenPeriod::Month) => "12 months",
            (Range::Default, _) => "30 days",
            (Range::Quarter, _) => "90 days",
            (Range::All, _) => "all time",
        }
    }
}

/// One level of the drill-down (spec §5.3): its query, the report last read for it, and the
/// selected row, kept by `RowKey`.
struct Level {
    /// `None` at the top, whose query is built from the settings each time it is read, so its
    /// range rolls with the clock; a drilled level's is `TokenQuery::drill`'s, fixed.
    query: Option<TokenQuery>,
    report: Option<TokenReport>,
    /// The words of this level's last report query, when it failed: its report is the last one
    /// read, drawn DIM under them (T2-22). A good report clears them, and so does leaving the
    /// level.
    failed: Option<String>,
    selected: Option<RowKey>,
    /// The first table line shown; clamped where it is used (R22).
    offset: usize,
    /// The report's column widths, found when it is first drawn.
    widths: table::Widths,
    /// The report's table lines, kept while the width, the row count and the active blocks'
    /// clocks stay the same.
    layout: table::Layout,
    /// The short forms of the report's models (T5-22), found when it is first drawn.
    models: OnceCell<ShortModels>,
}

impl Level {
    fn top() -> Level {
        Level::with(None)
    }

    fn with(query: Option<TokenQuery>) -> Level {
        Level {
            query,
            report: None,
            failed: None,
            selected: None,
            offset: 0,
            widths: table::Widths::default(),
            layout: table::Layout::default(),
            models: OnceCell::new(),
        }
    }

    /// The short forms of the report's models (T5-22): found the first time they are asked for,
    /// then kept until the next report.
    fn short_models(&self) -> Option<&ShortModels> {
        let report = self.report.as_ref()?;
        Some(self.models.get_or_init(|| ShortModels::of_report(report)))
    }
}

/// What the Tokens screen shows and asks for.
pub struct TokensState {
    /// `--provider`'s, else the default provider (spec §5.3).
    provider: ProviderId,
    /// The system zone (spec §5.3), read before the TUI started (B6).
    tz: TimeZone,
    period: TokenPeriod,
    grouping: TokenGrouping,
    range: Range,
    /// The filter (spec §5.3): `None` for every one of a kind.
    accounts: Option<Vec<AccountSel>>,
    models: Option<Vec<String>>,
    /// The top level first; never empty.
    levels: Vec<Level>,
    /// A query of the shown level is on the report lane.
    pending: bool,
    /// Asked to query again while `pending`: done once that query returns (B2).
    again: bool,
    /// A failed read's words, the banner while Tokens is open (spec §10.4, B12).
    error: Option<String>,
    /// The index's state, its refresh, and the timers that read the report again (§6.4).
    watch: IndexWatch,
    /// How far what lies below the total is scrolled (`J`/`K`, B25), and for which row: another
    /// row's starts at its top.
    below_from: usize,
    below_for: Option<RowKey>,
}

impl TokensState {
    /// By day and account over the default range, unfiltered (spec §5.3; `tagteam tokens`'s
    /// defaults).
    pub fn new(provider: ProviderId, tz: TimeZone) -> TokensState {
        TokensState {
            provider,
            tz,
            period: TokenPeriod::Day,
            grouping: TokenGrouping::Account,
            range: Range::Default,
            accounts: None,
            models: None,
            levels: vec![Level::top()],
            pending: false,
            again: false,
            error: None,
            watch: IndexWatch::default(),
            below_from: 0,
            below_for: None,
        }
    }

    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// The zone the screen's days and months are in (B6), which the TUI's other times use too.
    pub fn tz(&self) -> &TimeZone {
        &self.tz
    }

    #[cfg(test)]
    pub fn grouping(&self) -> TokenGrouping {
        self.grouping
    }

    #[cfg(test)]
    pub fn range(&self) -> Range {
        self.range
    }

    /// The report shown: the last one read for the shown level.
    pub fn report(&self) -> Option<&TokenReport> {
        self.level().report.as_ref()
    }

    /// The selected row's key, kept across refreshes (spec §5.3).
    pub fn selected(&self) -> Option<&RowKey> {
        self.level().selected.as_ref()
    }

    /// How deep the drill-down is: 0 at the top.
    pub fn depth(&self) -> usize {
        self.levels.len() - 1
    }

    /// Whether a query of the shown level is on the report lane.
    pub fn pending(&self) -> bool {
        self.pending
    }

    /// Whether the shown report is the last one read, the query after it having failed
    /// (T2-22).
    fn stale(&self) -> bool {
        self.level().failed.is_some()
    }

    /// T2-22's words while the shown report is the last one read.
    pub(crate) fn stale_words(&self) -> Option<&str> {
        self.level().failed.as_deref()
    }

    /// A failed read's words, for the banner (B12): the shown report's first, so the DIM report
    /// never loses them to a check, then a check's, else the last refresh's.
    pub fn error(&self) -> Option<&str> {
        let failed = self.level().failed.as_deref();
        failed
            .or(self.error.as_deref())
            .or_else(|| self.watch.error())
    }

    /// The index's state and the TUI's refresh of it.
    #[cfg(test)]
    pub fn watch(&self) -> &IndexWatch {
        &self.watch
    }

    /// The shown level's query at `now_ms`: the top's from the settings, its range ending now;
    /// a drilled level's as it was drilled.
    pub fn query(&self, now_ms: i64) -> TokenQuery {
        self.level()
            .query
            .clone()
            .unwrap_or_else(|| self.top_query(now_ms))
    }

    fn top_query(&self, now_ms: i64) -> TokenQuery {
        TokenQuery {
            provider: self.provider.clone(),
            period: self.period,
            grouping: self.grouping,
            since_ms: self.range.since_ms(self.period, now_ms, &self.tz),
            until_ms: None,
            accounts: self.accounts.clone(),
            models: self.models.clone(),
            session: None,
            tz: self.tz.clone(),
        }
    }

    fn level(&self) -> &Level {
        self.levels.last().expect("the top level is never left")
    }

    fn level_mut(&mut self) -> &mut Level {
        self.levels.last_mut().expect("the top level is never left")
    }

    /// The selected row of the shown report.
    pub fn selected_row(&self) -> Option<&TokenRow> {
        let level = self.level();
        let key = level.selected.as_ref()?;
        level.report.as_ref()?.rows.iter().find(|r| &r.key == key)
    }

    /// What `Enter` opens from the selected row: `sessions` from a period's row, `models` from
    /// a session's, and nothing from a session's per-model rows (spec §5.3).
    pub fn drill_to(&self) -> Option<&'static str> {
        let query = self.report()?.query.drill(self.selected_row()?)?;
        Some(if query.session.is_some() {
            "models"
        } else {
            "sessions"
        })
    }

    /// The selected row's place in the shown report.
    pub fn selected_index(&self) -> Option<usize> {
        let level = self.level();
        let key = level.selected.as_ref()?;
        level
            .report
            .as_ref()?
            .rows
            .iter()
            .position(|r| &r.key == key)
    }

    /// Takes a new report for the shown level. The selection stays on its `RowKey`; when that
    /// row is gone it moves to the row now at its old place, or the last row (spec §5.3). A
    /// report with no rows keeps the key, for the next one that has the row.
    pub fn apply(&mut self, report: TokenReport) {
        let old = self.selected_index();
        let level = self.level_mut();
        let kept = level
            .selected
            .as_ref()
            .is_some_and(|k| report.rows.iter().any(|r| &r.key == k));
        if !kept && !report.rows.is_empty() {
            let at = old.unwrap_or(0).min(report.rows.len() - 1);
            level.selected = Some(report.rows[at].key.clone());
        }
        level.report = Some(report);
        level.failed = None;
        level.widths = table::Widths::default();
        level.layout = table::Layout::default();
        level.models = OnceCell::new();
    }

    /// Moves the selection `by` rows, stopping at either end.
    fn step(&mut self, by: isize) {
        let at = self.selected_index().unwrap_or(0) as isize;
        let level = self.level_mut();
        let Some(rows) = level.report.as_ref().map(|r| &r.rows) else {
            return;
        };
        if rows.is_empty() {
            return;
        }
        let to = (at.saturating_add(by)).clamp(0, rows.len() as isize - 1) as usize;
        level.selected = Some(rows[to].key.clone());
    }

    /// Back to the top level, whose report is read anew: the query changed (B8). The selected
    /// key stays, for a row the new report still has, and so does the last report, until the
    /// new one arrives (redesign §8, VIS-10).
    fn reset(&mut self) {
        self.levels.truncate(1);
        self.levels[0].widths = table::Widths::default();
        self.levels[0].layout = table::Layout::default();
    }
}

/// A query of the shown level under a new generation: whatever is on the report lane is for
/// another query, or older, and is dropped when it returns (spec §4.3).
pub(crate) fn load(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    state.gens.report += 1;
    let t = &mut state.tokens;
    t.pending = true;
    t.again = false;
    let cmd = Cmd::LoadReport {
        r#gen: state.gens.report,
        query: t.query(now_ms),
    };
    index::on_load(state, now_ms);
    vec![cmd]
}

/// The same query again (spec §6.4's triggers). One already on the lane is let finish, so a
/// slow query is never starved by newer ones, and is followed by one more (B2).
pub(crate) fn requery(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.tokens.pending {
        state.tokens.again = true;
        return vec![];
    }
    load(state, now_ms)
}

/// Tokens opens: the report on screen may be stale, so it is read again (B19).
pub(crate) fn on_enter(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    requery(state, now_ms)
}

/// A report from the report lane. One of an older generation is dropped. A failure keeps the
/// report on screen under the banner (spec §10.4). A query asked for meanwhile goes out now.
pub(crate) fn on_report(
    state: &mut State,
    r#gen: u64,
    result: Result<TokenReport, SourceError>,
    now_ms: i64,
) -> Vec<Cmd> {
    if r#gen != state.gens.report {
        return vec![];
    }
    let t = &mut state.tokens;
    t.pending = false;
    let read = result.is_ok();
    match result {
        Ok(report) => {
            t.error = None;
            t.apply(report);
        }
        Err(e) => {
            t.level_mut().failed = Some(format!(
                "couldn't read the token index ({}) · r retries",
                e.message
            ));
        }
    }
    index::on_report(state, now_ms, read);
    if state.screen == Screen::Tokens {
        state.dirty = true;
    }
    let again = std::mem::take(&mut state.tokens.again);
    if again && state.screen == Screen::Tokens && state.quitting.is_none() {
        return requery(state, now_ms);
    }
    vec![]
}

/// The Tokens screen's keys (spec §5.6): `p` period, `g` group, `f` filter, `[` `]` range,
/// `Enter` down a level, `Esc`/`Backspace` up one, the rows, which keep the selected one in view,
/// and `J`/`K`, which scroll what lies below the total (B25).
pub fn handle_key(state: &mut State, key: KeyEvent, now_ms: i64) -> Vec<Cmd> {
    let body = crate::view::body_at(state);
    let page = table::page(state, body, now_ms).max(1) as isize;
    let t = &mut state.tokens;
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => t.step(1),
        KeyCode::Char('k') | KeyCode::Up => t.step(-1),
        KeyCode::PageDown => t.step(page),
        KeyCode::PageUp => t.step(-page),
        KeyCode::Home => t.step(isize::MIN / 2),
        KeyCode::End => t.step(isize::MAX / 2),
        KeyCode::Char('p') => {
            t.period = match t.period {
                TokenPeriod::Day => TokenPeriod::Month,
                TokenPeriod::Month => TokenPeriod::Session,
                TokenPeriod::Session => TokenPeriod::Block,
                TokenPeriod::Block => TokenPeriod::Day,
            };
            t.reset();
            return load(state, now_ms);
        }
        KeyCode::Char('g') => {
            t.grouping = match t.grouping {
                TokenGrouping::None => TokenGrouping::Account,
                TokenGrouping::Account => TokenGrouping::Model,
                TokenGrouping::Model => TokenGrouping::AccountModel,
                TokenGrouping::AccountModel => TokenGrouping::None,
            };
            t.reset();
            return load(state, now_ms);
        }
        KeyCode::Char('[') | KeyCode::Char(']') => {
            let by = if key.code == KeyCode::Char('[') {
                -1
            } else {
                1
            };
            t.range = t.range.step(by);
            t.reset();
            return load(state, now_ms);
        }
        KeyCode::Char('f') => return filter::open(state),
        KeyCode::Char('J') | KeyCode::Char('K') => {
            let by = if key.code == KeyCode::Char('J') {
                1
            } else {
                -1
            };
            table::scroll_below(state, body, now_ms, by);
            return vec![];
        }
        KeyCode::Enter => return drill(state, now_ms),
        KeyCode::Esc | KeyCode::Backspace => return up(state, now_ms),
        _ => return vec![],
    }
    table::follow(state, body, now_ms);
    vec![]
}

/// `Enter`: the selected row's next level down (`TokenQuery::drill`), from the query its report
/// answered. At a session's per-model rows there is none, and nothing happens.
fn drill(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    let t = &mut state.tokens;
    let (Some(report), Some(row)) = (t.report(), t.selected_row()) else {
        return vec![];
    };
    let Some(query) = report.query.drill(row) else {
        return vec![];
    };
    t.levels.push(Level::with(Some(query)));
    load(state, now_ms)
}

/// `Esc` or `Backspace`: back one level, to its report and its selection as they were, read
/// again.
fn up(state: &mut State, now_ms: i64) -> Vec<Cmd> {
    if state.tokens.levels.len() == 1 {
        return vec![];
    }
    state.tokens.levels.pop();
    load(state, now_ms)
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;
    use tagteam_engine::tokens::PeriodKey;

    use super::*;
    use crate::msg::Msg;
    use crate::testutil::*;
    use crate::update::update;

    /// 2025-10-01T00:00:00Z: the first day of the eleventh month before `NOW_MS`'s.
    const OCTOBER_2025: i64 = 1_759_276_800_000;
    /// 2026-09-20T00:00:00Z, the day before `NOW_MS`'s.
    const YESTERDAY: i64 = 1_789_862_400_000;

    /// The query the one `LoadReport` in `cmds` asks for, and its generation.
    fn asked(cmds: &[Cmd]) -> (u64, TokenQuery) {
        match cmds {
            [Cmd::LoadReport { r#gen, query }] => (*r#gen, query.clone()),
            other => panic!("{other:?}"),
        }
    }

    /// Answers the query in `cmds` with `rows`, as the report lane would.
    fn answer(state: &mut State, cmds: &[Cmd], rows: Vec<TokenRow>) -> Vec<Cmd> {
        let (r#gen, query) = asked(cmds);
        let result = Ok(token_report(query, rows));
        update(state, Msg::TokenReport { r#gen, result }, NOW_MS)
    }

    fn row(period: PeriodKey, position: u32) -> TokenRow {
        token_row(
            period,
            Some(row_account(position)),
            None,
            counts(100, 10, 1_000),
            Some(1_000_000),
        )
    }

    fn session(id: &str, position: u32) -> TokenRow {
        let period = PeriodKey::Session {
            session_id: id.into(),
            project: Some("/w".into()),
            first_ms: YESTERDAY + 3_600_000,
            last_ms: YESTERDAY + 7_200_000,
        };
        row(period, position)
    }

    /// Tokens, opened, showing yesterday and the day before for accounts 1 and 2.
    fn showing_days() -> State {
        let mut state = fresh(Screen::Accounts);
        let cmds = update(&mut state, ch('2'), NOW_MS);
        answer(
            &mut state,
            &cmds,
            vec![row(day(1), 1), row(day(1), 2), row(day(2), 1)],
        );
        state
    }

    #[test]
    fn opening_tokens_reads_the_last_30_days_by_day_and_account() {
        // Spec §5.3: `tagteam tokens`'s defaults.
        let mut state = fresh(Screen::Accounts);
        let cmds = update(&mut state, ch('2'), NOW_MS);
        assert_eq!(
            cmds,
            [Cmd::LoadReport {
                r#gen: 1,
                query: token_query(TokenPeriod::Day),
            }]
        );
        assert!(state.tokens.pending());
        let mut started = fresh(Screen::Tokens);
        let cmds = started.start();
        assert!(
            matches!(
                cmds.as_slice(),
                [
                    Cmd::LoadAccounts { r#gen: 1 },
                    Cmd::LoadReport { r#gen: 1, .. }
                ]
            ),
            "{cmds:?}"
        );
    }

    #[test]
    fn the_period_cycles_and_a_month_report_spans_twelve_calendar_months() {
        let mut state = showing_days();
        let (_, month) = asked(&update(&mut state, ch('p'), NOW_MS));
        assert_eq!(
            (month.period, month.since_ms),
            (TokenPeriod::Month, Some(OCTOBER_2025)),
            "the first of the eleventh month back (token report §11.1)"
        );
        assert_eq!(state.tokens.range().label(TokenPeriod::Month), "12 months");
        let mut seen = vec![];
        for _ in 0..3 {
            let (_, q) = asked(&update(&mut state, ch('p'), NOW_MS));
            seen.push((q.period, q.since_ms));
        }
        assert_eq!(
            seen,
            [
                (TokenPeriod::Session, Some(NOW_MS - 30 * DAY_MS)),
                (TokenPeriod::Block, Some(NOW_MS - 30 * DAY_MS)),
                (TokenPeriod::Day, Some(NOW_MS - 30 * DAY_MS)),
            ]
        );
    }

    #[test]
    fn the_range_cycles_through_7_30_and_90_days_and_all() {
        let mut state = showing_days();
        let mut seen = vec![];
        for _ in 0..4 {
            let (_, q) = asked(&update(&mut state, ch(']'), NOW_MS));
            seen.push((q.since_ms, q.until_ms));
        }
        assert_eq!(
            seen,
            [
                (Some(NOW_MS - 90 * DAY_MS), None),
                (None, None),
                (Some(NOW_MS - 7 * DAY_MS), None),
                (Some(NOW_MS - 30 * DAY_MS), None),
            ]
        );
        let (_, q) = asked(&update(&mut state, ch('['), NOW_MS));
        assert_eq!(q.since_ms, Some(NOW_MS - 7 * DAY_MS));
        assert_eq!(state.tokens.range().label(TokenPeriod::Day), "7 days");
    }

    #[test]
    fn the_grouping_cycles_and_a_new_query_starts_over_at_the_top() {
        let mut state = showing_days();
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 1)]);
        assert_eq!(state.tokens.depth(), 1);
        let mut seen = vec![];
        let mut last = vec![];
        for _ in 0..4 {
            last = update(&mut state, ch('g'), NOW_MS);
            let (_, q) = asked(&last);
            seen.push(q.grouping);
            assert_eq!((q.period, q.session), (TokenPeriod::Day, None));
        }
        assert_eq!(
            seen,
            [
                TokenGrouping::Model,
                TokenGrouping::AccountModel,
                TokenGrouping::None,
                TokenGrouping::Account,
            ]
        );
        assert_eq!(state.tokens.depth(), 0, "back at the top (B8)");
        assert_eq!(
            state.tokens.report().map(|r| r.rows.len()),
            Some(3),
            "the last report stays until the new one comes (VIS-10)"
        );
        assert!(state.tokens.pending());
        answer(&mut state, &last, vec![row(day(1), 1)]);
        assert_eq!(
            state
                .tokens
                .report()
                .map(|r| (r.rows.len(), r.query.grouping)),
            Some((1, TokenGrouping::Account)),
            "the next report replaces it"
        );
        assert!(!state.tokens.pending());
    }

    #[test]
    fn enter_goes_from_a_day_to_its_sessions_and_from_a_session_to_its_models_and_no_further() {
        let mut state = showing_days();
        assert_eq!(state.tokens.drill_to(), Some("sessions"));
        let (_, sessions) = asked(&update(&mut state, key(KeyCode::Enter), NOW_MS));
        assert_eq!(sessions.period, TokenPeriod::Session);
        assert_eq!(
            (sessions.since_ms, sessions.until_ms),
            (Some(YESTERDAY), Some(YESTERDAY + DAY_MS)),
            "the day's span"
        );
        assert_eq!(
            sessions.accounts,
            Some(vec![AccountSel::Account(id(1))]),
            "the row's account"
        );
        assert!(state.tokens.report().is_none(), "`loading…`");
        let asked_for = [Cmd::LoadReport {
            r#gen: state.gens.report,
            query: sessions,
        }];
        answer(&mut state, &asked_for, vec![session("s1", 1)]);
        assert_eq!(state.tokens.drill_to(), Some("models"));
        let (_, models) = asked(&update(&mut state, key(KeyCode::Enter), NOW_MS));
        assert_eq!(
            (models.session.as_deref(), models.grouping, models.since_ms),
            (Some("s1"), TokenGrouping::AccountModel, None),
            "the whole session, by account and model"
        );
        let per_model = PeriodKey::Session {
            session_id: "s1".into(),
            project: Some("/w".into()),
            first_ms: YESTERDAY + 3_600_000,
            last_ms: YESTERDAY + 7_200_000,
        };
        let opus = token_row(
            per_model,
            Some(row_account(1)),
            Some("claude-opus-5-5"),
            counts(1, 2, 3),
            None,
        );
        let asked_for = [Cmd::LoadReport {
            r#gen: state.gens.report,
            query: models,
        }];
        answer(&mut state, &asked_for, vec![opus]);
        assert_eq!(state.tokens.depth(), 2);
        assert_eq!(state.tokens.drill_to(), None);
        assert!(
            update(&mut state, key(KeyCode::Enter), NOW_MS).is_empty(),
            "Enter does nothing at the bottom"
        );
        assert_eq!(state.tokens.depth(), 2);
    }

    #[test]
    fn esc_and_backspace_go_back_a_level_to_its_report_and_selection() {
        let mut state = showing_days();
        update(&mut state, ch('j'), NOW_MS);
        let selected = state.tokens.selected().cloned();
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 2)]);
        let back = update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert_eq!(state.tokens.depth(), 0);
        assert_eq!(state.tokens.selected(), selected.as_ref());
        assert_eq!(
            state.tokens.report().map(|r| r.rows.len()),
            Some(3),
            "shown at once, while it is read again"
        );
        let (_, q) = asked(&back);
        assert_eq!(q, token_query(TokenPeriod::Day));
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 2)]);
        update(&mut state, key(KeyCode::Backspace), NOW_MS);
        assert_eq!(state.tokens.depth(), 0);
        assert!(
            update(&mut state, key(KeyCode::Esc), NOW_MS).is_empty(),
            "the top has no level above it"
        );
    }

    #[test]
    fn the_selection_follows_its_row_key_across_a_refresh() {
        // Spec §5.3: a refresh that adds a row above the selected one keeps the selection on
        // the same row; when that row is gone, the row now at its place is selected.
        let mut state = showing_days();
        update(&mut state, ch('j'), NOW_MS);
        let key2 = row(day(1), 2).key;
        assert_eq!(state.tokens.selected(), Some(&key2));
        let again = on_enter(&mut state, NOW_MS);
        answer(
            &mut state,
            &again,
            vec![
                row(day(0), 1),
                row(day(1), 1),
                row(day(1), 2),
                row(day(2), 1),
            ],
        );
        assert_eq!(state.tokens.selected(), Some(&key2));
        assert_eq!(state.tokens.selected_index(), Some(2));
        let again = on_enter(&mut state, NOW_MS);
        answer(
            &mut state,
            &again,
            vec![row(day(0), 1), row(day(1), 1), row(day(2), 1)],
        );
        assert_eq!(state.tokens.selected(), Some(&row(day(2), 1).key));
        let again = on_enter(&mut state, NOW_MS);
        answer(&mut state, &again, vec![]);
        assert_eq!(
            state.tokens.selected(),
            Some(&row(day(2), 1).key),
            "an empty report keeps the key for the next"
        );
    }

    #[test]
    fn j_and_k_move_the_selection_and_stop_at_either_end() {
        let mut state = showing_days();
        let keys = |state: &State| state.tokens.selected().cloned();
        assert_eq!(keys(&state), Some(row(day(1), 1).key));
        for _ in 0..5 {
            update(&mut state, ch('j'), NOW_MS);
        }
        assert_eq!(keys(&state), Some(row(day(2), 1).key));
        update(&mut state, key(KeyCode::Home), NOW_MS);
        assert_eq!(keys(&state), Some(row(day(1), 1).key));
        update(&mut state, ch('k'), NOW_MS);
        assert_eq!(keys(&state), Some(row(day(1), 1).key));
        update(&mut state, key(KeyCode::End), NOW_MS);
        assert_eq!(keys(&state), Some(row(day(2), 1).key));
    }

    #[test]
    fn a_report_of_an_older_generation_is_dropped() {
        // Spec §4.3: a slow query never overwrites a newer one.
        let mut state = showing_days();
        let older = update(&mut state, ch('p'), NOW_MS);
        let newer = update(&mut state, ch('p'), NOW_MS);
        let month = PeriodKey::Month {
            year: 2026,
            month: 9,
        };
        assert!(answer(&mut state, &older, vec![row(month, 1)]).is_empty());
        assert_eq!(
            state.tokens.report().map(|r| r.query.period),
            Some(TokenPeriod::Day),
            "the month report is dropped, and the last one stays"
        );
        assert!(
            state.tokens.pending(),
            "the session report is still to come"
        );
        answer(&mut state, &newer, vec![session("s1", 1)]);
        assert_eq!(
            state.tokens.report().map(|r| r.query.period),
            Some(TokenPeriod::Session)
        );
    }

    #[test]
    fn a_query_asked_for_while_one_runs_follows_it_once() {
        // B2: the slow query is let finish, then the screen is read once more.
        let mut state = showing_days();
        let first = on_enter(&mut state, NOW_MS);
        assert!(on_enter(&mut state, NOW_MS).is_empty());
        assert!(on_enter(&mut state, NOW_MS).is_empty());
        let follow = answer(&mut state, &first, vec![row(day(1), 1)]);
        let (r#gen, _) = asked(&follow);
        assert_eq!(r#gen, state.gens.report);
        assert!(answer(&mut state, &follow, vec![row(day(1), 1)]).is_empty());
        assert!(!state.tokens.pending());
    }

    #[test]
    fn a_failed_report_keeps_the_last_one_under_a_banner_shown_on_tokens_only() {
        let mut state = showing_days();
        let again = on_enter(&mut state, NOW_MS);
        let (r#gen, _) = asked(&again);
        let failed = Err(SourceError {
            kind: "store".into(),
            message: "the store is locked".into(),
        });
        update(
            &mut state,
            Msg::TokenReport {
                r#gen,
                result: failed,
            },
            NOW_MS,
        );
        assert_eq!(state.tokens.report().map(|r| r.rows.len()), Some(3));
        assert_eq!(
            draw(&state, 80, 24)[3],
            " ! couldn't read the token index (the store is locked) · r retries"
        );
        update(&mut state, ch('1'), NOW_MS);
        assert!(
            !draw(&state, 80, 24)
                .iter()
                .any(|l| l.contains("the store is locked")),
            "a token read's failure is Tokens' own"
        );
        let again = update(&mut state, ch('2'), NOW_MS);
        answer(&mut state, &again, vec![row(day(1), 1)]);
        assert_eq!(state.tokens.error(), None, "the next good read clears it");
    }

    #[test]
    fn a_failed_drilled_query_is_that_level_s_and_going_back_clears_it() {
        // T2-22: the report and its words belong to the level whose query failed.
        let mut state = showing_days();
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        let (r#gen, _) = asked(&down);
        let failed = Err(SourceError {
            kind: "store".into(),
            message: "the index is locked".into(),
        });
        update(
            &mut state,
            Msg::TokenReport {
                r#gen,
                result: failed,
            },
            NOW_MS,
        );
        assert_eq!(
            state.tokens.error(),
            Some("couldn't read the token index (the index is locked) · r retries")
        );
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        assert_eq!(state.tokens.depth(), 0);
        assert_eq!(state.tokens.error(), None);
        let buf = draw_buffer(&state, 80, 24, NOW_MS);
        // The total line's `TOTAL`: the head's is muted, which tiers 0 and 1 draw DIM.
        let y = text_of(&buf)
            .iter()
            .position(|l| l.starts_with(" TOTAL"))
            .unwrap() as u16;
        assert!(
            (1..6).all(|x| !buf[(x, y)].modifier.contains(ratatui::style::Modifier::DIM)),
            "the top report is plain"
        );
    }

    #[test]
    fn help_on_tokens_shows_each_key_whole_at_80_columns() {
        // R21: the help box is 72 columns inside at 80; no key or description runs past it. The
        // panel is tall enough here to show every row: its scrolling is `help`'s own test.
        let mut state = showing_days();
        update(&mut state, ch('?'), NOW_MS);
        let lines = draw(&state, 80, 60);
        let rows = crate::keys::KEYS
            .iter()
            .filter(|row| row.screen.is_none_or(|s| s == Screen::Tokens));
        for row in rows {
            assert!(
                lines
                    .iter()
                    .any(|l| l.contains(row.keys) && l.contains(row.does)),
                "{} {}: {lines:?}",
                row.keys,
                row.does
            );
        }
    }

    #[test]
    fn the_footer_says_what_enter_opens() {
        let mut state = showing_days();
        assert_eq!(
            draw(&state, 80, 24)[23],
            footer_row(
                "⏎ sessions  p period  g group  f filter  [ ] range  r refresh",
                80
            ),
            "Esc does nothing at the top, so the footer leaves it out"
        );
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 1)]);
        assert_eq!(
            draw(&state, 80, 24)[23],
            footer_row(
                "⏎ models  p period  g group  f filter  [ ] range  Esc back",
                80
            ),
            "`r refresh` gives way: the pinned two keep their room"
        );
        let down = update(&mut state, key(KeyCode::Enter), NOW_MS);
        answer(&mut state, &down, vec![session("s1", 1)]);
        assert_eq!(state.tokens.depth(), 2);
        assert_eq!(
            draw(&state, 80, 24)[23],
            footer_row(
                "p period  g group  f filter  [ ] range  Esc back  r refresh",
                80
            )
        );
    }
}
