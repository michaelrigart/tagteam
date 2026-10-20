//! The Tokens screen's filter (spec §5.3): a dialog of every account and model the index holds,
//! whatever the filter is now, each chosen or not. A multi-select overlay within part A's: it
//! takes every key but Ctrl-C and Ctrl-Z (R9), wraps a choice wider than itself under its mark
//! onto `CHOICE_ROWS` at most as part A's choose dialog does (B27, B29), and places its list
//! from the cursor each frame (R22).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::text::{Line, Span};
use tagteam_engine::tokens::{
    AccountSel, RowAccount, TokenGrouping, TokenPeriod, TokenQuery, TokenReport,
};

use super::table::account_name;
use super::{TokensState, load};
use crate::msg::Cmd;
use crate::overlay::{CHOICE_ROWS, Overlay, boxed};
use crate::source::SourceError;
use crate::state::State;
use crate::view::{scrolled_windows, wrap_rows};

/// Shown until the inventory arrives.
pub const LOADING: &str = "loading the accounts and models the index holds…";
pub const NO_ACCOUNT: &str = "Choose at least one account.";
pub const NO_MODEL: &str = "Choose at least one model.";
const KEYS: &str = "Space choose · a all · Enter apply · Esc cancel";

/// What a choice selects.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Account(AccountSel),
    Model(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice {
    kind: Kind,
    label: String,
    on: bool,
}

/// The filter dialog's state.
#[derive(Debug)]
pub struct FilterDialog {
    /// `None` until the inventory arrives.
    choices: Option<Vec<Choice>>,
    /// The choice the cursor is on.
    cursor: usize,
    /// Why Enter was refused, or why the choices could not be read.
    error: Option<String>,
}

/// The query whose rows are the inventory (spec §5.3): by month, account and model, over all
/// time, unfiltered, so the choices never depend on the filter in force.
fn inventory(t: &TokensState) -> TokenQuery {
    TokenQuery {
        provider: t.provider.clone(),
        period: TokenPeriod::Month,
        grouping: TokenGrouping::AccountModel,
        since_ms: None,
        until_ms: None,
        accounts: None,
        models: None,
        session: None,
        tz: t.tz.clone(),
    }
}

/// `f`: the dialog opens while its inventory is read on the report lane.
pub(super) fn open(state: &mut State) -> Vec<Cmd> {
    state.overlay = Some(Overlay::Filter(FilterDialog {
        choices: None,
        cursor: 0,
        error: None,
    }));
    state.gens.inventory += 1;
    vec![Cmd::LoadInventory {
        r#gen: state.gens.inventory,
        query: inventory(&state.tokens),
    }]
}

/// Where an account comes in the dialog: as a report orders them (token report §10.1), by
/// position, then removed accounts, then unattributed.
fn rank(a: &RowAccount) -> (u8, u32, String) {
    match a {
        RowAccount::Account { id, position, .. } => (0, *position, id.as_str().to_owned()),
        RowAccount::Removed { id } => (1, 0, id.as_str().to_owned()),
        RowAccount::Unattributed => (2, 0, String::new()),
    }
}

fn sel(a: &RowAccount) -> AccountSel {
    match a {
        RowAccount::Account { id, .. } | RowAccount::Removed { id } => {
            AccountSel::Account(id.clone())
        }
        RowAccount::Unattributed => AccountSel::Unattributed,
    }
}

/// The choices (spec §5.3): the inventory's accounts (current, removed and unattributed) and
/// the Tokens provider's accounts in the accounts view, then the inventory's models, with
/// whatever the filter in force names that neither has. Each is chosen as the filter is: all of
/// a kind for `None`.
fn choices(state: &State, report: &TokenReport) -> Vec<Choice> {
    let t = &state.tokens;
    let mut accounts: Vec<RowAccount> = Vec::new();
    let mut add = |a: RowAccount| {
        if !accounts.iter().any(|b| sel(b) == sel(&a)) {
            accounts.push(a);
        }
    };
    for row in &report.rows {
        row.account
            .iter()
            .chain(&row.accounts)
            .cloned()
            .for_each(&mut add);
    }
    if let Some(snap) = state.accounts.snapshot() {
        for v in snap
            .providers
            .iter()
            .filter(|p| &p.provider == t.provider())
            .flat_map(|p| &p.accounts)
        {
            add(RowAccount::Account {
                id: v.row.id.clone(),
                position: v.row.position,
                label: v.row.label.clone(),
                alias: v.row.alias.clone(),
            });
        }
    }
    for s in t.accounts.iter().flatten() {
        add(match s {
            AccountSel::Account(id) => RowAccount::Removed { id: id.clone() },
            AccountSel::Unattributed => RowAccount::Unattributed,
        });
    }
    accounts.sort_by_key(rank);
    let mut models: Vec<String> = report
        .rows
        .iter()
        .flat_map(|r| r.model.iter().chain(&r.models).cloned())
        .chain(t.models.iter().flatten().cloned())
        .collect();
    models.sort();
    models.dedup();
    let account = |a: RowAccount| Choice {
        on: t.accounts.as_ref().is_none_or(|f| f.contains(&sel(&a))),
        label: account_name(&a),
        kind: Kind::Account(sel(&a)),
    };
    let model = |m: String| Choice {
        on: t.models.as_ref().is_none_or(|f| f.contains(&m)),
        label: m.clone(),
        kind: Kind::Model(m),
    };
    accounts
        .into_iter()
        .map(account)
        .chain(models.into_iter().map(model))
        .collect()
}

/// The inventory from the report lane: the dialog's choices, if it is still open and this is
/// its newest read.
pub(crate) fn on_inventory(
    state: &mut State,
    r#gen: u64,
    result: Result<TokenReport, SourceError>,
) {
    if r#gen != state.gens.inventory || !matches!(state.overlay, Some(Overlay::Filter(_))) {
        return;
    }
    let read = result.map(|report| choices(state, &report));
    if let Some(Overlay::Filter(dialog)) = &mut state.overlay {
        match read {
            Ok(choices) => dialog.choices = Some(choices),
            Err(e) => dialog.error = Some(format!("the choices could not be read: {}", e.message)),
        }
        state.dirty = true;
    }
}

/// What the choices of a kind select: `None` when every one is chosen, so that what the index
/// gains later is in too; else exactly those chosen (spec §5.3).
fn selection<T>(choices: &[&Choice], pick: impl Fn(&Kind) -> Option<T>) -> Option<Vec<T>> {
    if choices.iter().all(|c| c.on) {
        return None;
    }
    Some(
        choices
            .iter()
            .filter(|c| c.on)
            .filter_map(|c| pick(&c.kind))
            .collect(),
    )
}

/// A key in the dialog: the cursor (`j`/`k`, arrows, `Home`/`End`), Space, `a` for all or none
/// of the cursor's kind, Enter to apply, Esc to cancel. Enter refuses a kind with none chosen (token report §10.1: `Some` of an
/// empty list matches nothing).
pub(crate) fn handle_key(
    state: &mut State,
    mut dialog: FilterDialog,
    key: KeyEvent,
    now_ms: i64,
) -> (Option<Overlay>, Vec<Cmd>) {
    if key.code == KeyCode::Esc {
        return (None, vec![]);
    }
    let Some(choices) = dialog.choices.as_mut() else {
        return (Some(Overlay::Filter(dialog)), vec![]);
    };
    let last = choices.len().saturating_sub(1);
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => dialog.cursor = dialog.cursor.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => dialog.cursor = (dialog.cursor + 1).min(last),
        KeyCode::Home => dialog.cursor = 0,
        KeyCode::End => dialog.cursor = last,
        KeyCode::Char(' ') if !choices.is_empty() => {
            choices[dialog.cursor].on = !choices[dialog.cursor].on;
            dialog.error = None;
        }
        KeyCode::Char('a') if !choices.is_empty() => {
            let account = matches!(choices[dialog.cursor].kind, Kind::Account(_));
            let mine = |c: &Choice| matches!(c.kind, Kind::Account(_)) == account;
            let every = choices.iter().filter(|c| mine(c)).all(|c| c.on);
            for c in choices.iter_mut().filter(|c| mine(c)) {
                c.on = !every;
            }
            dialog.error = None;
        }
        KeyCode::Enter => {
            let (accounts, models): (Vec<&Choice>, Vec<&Choice>) = choices
                .iter()
                .partition(|c| matches!(c.kind, Kind::Account(_)));
            let none = |kind: &[&Choice]| !kind.is_empty() && kind.iter().all(|c| !c.on);
            if none(&accounts) || none(&models) {
                let refusal = if none(&accounts) {
                    NO_ACCOUNT
                } else {
                    NO_MODEL
                };
                dialog.error = Some(refusal.to_owned());
                return (Some(Overlay::Filter(dialog)), vec![]);
            }
            let accounts = selection(&accounts, |k| match k {
                Kind::Account(s) => Some(s.clone()),
                Kind::Model(_) => None,
            });
            let models = selection(&models, |k| match k {
                Kind::Model(m) => Some(m.clone()),
                Kind::Account(_) => None,
            });
            let t = &mut state.tokens;
            t.accounts = accounts;
            t.models = models;
            t.reset();
            return (None, load(state, now_ms));
        }
        _ => {}
    }
    (Some(Overlay::Filter(dialog)), vec![])
}

/// The dialog (B7, B27, B29): the accounts, then the models, each `[x]` when chosen and wrapped
/// under its mark, the cursor's in reverse video; the list placed from the cursor so that its
/// whole choice shows, and the keys always below it. A choice and a failure take `CHOICE_ROWS`
/// at most, the rest cut with `…`, so the dialog's own lines and the cursor's choice always fit
/// at 80×24.
pub(crate) fn render(state: &State, dialog: &FilterDialog, frame: &mut Frame) {
    let area = frame.area();
    let w = 64u16.min(area.width.saturating_sub(4));
    let inner = usize::from(w).saturating_sub(4).max(1);
    let mut tail: Vec<Line<'static>> = Vec::new();
    if let Some(error) = &dialog.error {
        let style = state
            .theme
            .severity(tagteam_engine::words::Severity::Critical);
        tail.extend(
            wrap_rows(error, inner, CHOICE_ROWS)
                .into_iter()
                .map(|row| Line::from(Span::styled(row, style))),
        );
    }
    let Some(choices) = &dialog.choices else {
        let mut lines: Vec<Line<'static>> = Vec::new();
        if dialog.error.is_none() {
            lines.extend(
                wrap_rows(LOADING, inner, CHOICE_ROWS)
                    .into_iter()
                    .map(Line::from),
            );
        }
        lines.extend(tail);
        lines.push(Line::from("Esc cancel"));
        boxed(frame, area, " Filter ", lines, w);
        return;
    };
    tail.extend(
        wrap_rows(KEYS, inner, CHOICE_ROWS)
            .into_iter()
            .map(Line::from),
    );
    // Each choice's rows, after its kind's heading when it is the first of its kind.
    let mut blocks: Vec<Vec<Line<'static>>> = Vec::new();
    let mut at = 0;
    for (i, c) in choices.iter().enumerate() {
        let account = matches!(c.kind, Kind::Account(_));
        let mut block = Vec::new();
        if i == 0 || matches!(choices[i - 1].kind, Kind::Account(_)) != account {
            let heading = if account { "Accounts" } else { "Models" };
            block.push(Line::from(Span::styled(heading, state.theme.strong())));
        }
        let mark = if c.on { "[x] " } else { "[ ] " };
        for (n, row) in wrap_rows(&c.label, inner.saturating_sub(4).max(1), CHOICE_ROWS)
            .into_iter()
            .enumerate()
        {
            let lead = if n == 0 { mark } else { "    " };
            let line = Line::from(format!("{lead}{row}"));
            block.push(if i == dialog.cursor {
                line.style(state.theme.selection())
            } else {
                line
            });
        }
        if i == dialog.cursor {
            at = blocks.len();
        }
        blocks.push(block);
    }
    // The box: the list, the tail, the border, and a row's margin above and below.
    let room = usize::from(area.height)
        .saturating_sub(tail.len() + 4)
        .max(1);
    let heights: Vec<usize> = blocks.iter().map(Vec::len).collect();
    // From the first choice that keeps the cursor's whole choice in view.
    let mut first = at;
    let mut used = heights.get(at).copied().unwrap_or(0);
    while first > 0 && used + heights[first - 1] <= room {
        first -= 1;
        used += heights[first];
    }
    let shown = scrolled_windows(&heights, first, room);
    let mut lines: Vec<Line<'static>> = blocks
        .into_iter()
        .enumerate()
        .filter(|(i, _)| shown.contains(i))
        .flat_map(|(_, block)| block)
        .collect();
    lines.extend(tail);
    boxed(frame, area, " Filter ", lines, w);
}

#[cfg(test)]
mod tests {
    use tagteam_core::AccountId;
    use tagteam_engine::tokens::PeriodKey;

    use super::*;
    use crate::msg::Msg;
    use crate::state::Screen;
    use crate::testutil::*;
    use crate::update::update;

    const OPUS: &str = "claude-opus-5-5";
    const SONNET: &str = "claude-sonnet-5-5";

    fn removed() -> RowAccount {
        RowAccount::Removed {
            id: AccountId::from_string("id-9"),
        }
    }

    /// The index: account 1 with opus and sonnet, a removed account with opus, unattributed
    /// sonnet; by month, account and model.
    fn indexed(query: TokenQuery) -> TokenReport {
        let month = PeriodKey::Month {
            year: 2026,
            month: 9,
        };
        let row = |account: RowAccount, model: &str| {
            token_row(
                month.clone(),
                Some(account),
                Some(model),
                counts(1, 2, 3),
                None,
            )
        };
        token_report(
            query,
            vec![
                row(row_account(1), OPUS),
                row(row_account(1), SONNET),
                row(removed(), OPUS),
                row(RowAccount::Unattributed, SONNET),
            ],
        )
    }

    /// Tokens, its accounts 1 and 2 read, and the filter dialog open with its inventory.
    fn dialog() -> State {
        let mut state = fresh(Screen::Tokens);
        state.accounts.apply(snapshot(many(2)));
        state.start();
        let cmds = update(&mut state, ch('f'), NOW_MS);
        let [Cmd::LoadInventory { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let result = Ok(indexed(query.clone()));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        state
    }

    /// The dialog's choices: each label, and whether it is chosen.
    fn shown(state: &State) -> Vec<(String, bool)> {
        let Some(Overlay::Filter(d)) = &state.overlay else {
            panic!("no filter dialog");
        };
        d.choices
            .iter()
            .flatten()
            .map(|c| (c.label.clone(), c.on))
            .collect()
    }

    fn all(on: bool) -> Vec<(String, bool)> {
        [
            "1 a1@x.co",
            "2 a2@x.co",
            "removed id-9",
            "unattributed",
            OPUS,
            SONNET,
        ]
        .iter()
        .map(|l| ((*l).to_owned(), on))
        .collect()
    }

    /// The query that applying the dialog asks for.
    fn applied(state: &mut State) -> TokenQuery {
        let cmds = update(state, key(KeyCode::Enter), NOW_MS);
        let [Cmd::LoadReport { query, .. }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        query.clone()
    }

    #[test]
    fn f_opens_the_dialog_on_an_unfiltered_inventory() {
        let mut state = fresh(Screen::Tokens);
        state.start();
        let cmds = update(&mut state, ch('f'), NOW_MS);
        assert_eq!(
            cmds,
            [Cmd::LoadInventory {
                r#gen: 1,
                query: TokenQuery {
                    period: TokenPeriod::Month,
                    grouping: TokenGrouping::AccountModel,
                    since_ms: None,
                    ..token_query(TokenPeriod::Day)
                },
            }]
        );
        let lines = draw(&state, 80, 24);
        assert!(lines.iter().any(|l| l.contains(LOADING)), "{lines:?}");
    }

    #[test]
    fn the_choices_are_the_indexed_accounts_and_models_with_the_current_accounts() {
        // Spec §5.3: current, removed and unattributed accounts, the accounts view's (2 has no
        // usage yet), and the models; with no filter, every one is chosen.
        let state = dialog();
        assert_eq!(shown(&state), all(true));
        let lines = draw(&state, 80, 24);
        for text in [
            "Accounts",
            "[x] 1 a1@x.co",
            "[x] removed id-9",
            "Models",
            "[x] claude-sonnet-5-5",
        ] {
            assert!(lines.iter().any(|l| l.contains(text)), "{text}: {lines:?}");
        }
        assert!(lines.iter().any(|l| l.contains(KEYS)));
    }

    #[test]
    fn every_choice_of_a_kind_sends_none_and_fewer_send_exactly_them() {
        let mut state = dialog();
        let q = applied(&mut state);
        assert_eq!(
            (q.accounts, q.models),
            (None, None),
            "models indexed later are in too"
        );
        assert!(state.overlay.is_none());
        let mut state = dialog();
        for _ in 0..5 {
            update(&mut state, ch('j'), NOW_MS);
        }
        update(&mut state, ch(' '), NOW_MS);
        update(&mut state, key(KeyCode::Home), NOW_MS);
        update(&mut state, ch('k'), NOW_MS);
        update(&mut state, ch('j'), NOW_MS);
        update(&mut state, ch(' '), NOW_MS);
        let q = applied(&mut state);
        assert_eq!(q.models, Some(vec![OPUS.to_owned()]));
        assert_eq!(
            q.accounts,
            Some(vec![
                AccountSel::Account(id(1)),
                AccountSel::Account(AccountId::from_string("id-9")),
                AccountSel::Unattributed,
            ])
        );
        assert_eq!(state.tokens.depth(), 0);
    }

    #[test]
    fn the_choices_stay_the_whole_inventory_after_filtering_to_one_model() {
        let mut state = dialog();
        for _ in 0..5 {
            update(&mut state, ch('j'), NOW_MS);
        }
        update(&mut state, ch(' '), NOW_MS);
        applied(&mut state);
        let cmds = update(&mut state, ch('f'), NOW_MS);
        let [Cmd::LoadInventory { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        assert_eq!(query.models, None, "the inventory is never filtered");
        let result = Ok(indexed(query.clone()));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        let mut expected = all(true);
        expected[5].1 = false;
        assert_eq!(shown(&state), expected);
        let lines = draw(&state, 120, 40);
        assert!(lines.iter().any(|l| l.contains("[ ] claude-sonnet-5-5")));
        assert!(
            lines[1].contains("filter: all accounts · claude-opus-5-5"),
            "the header names the one model: {}",
            lines[1]
        );
    }

    #[test]
    fn none_of_a_kind_is_refused_in_the_dialog() {
        let mut state = dialog();
        for _ in 0..5 {
            update(&mut state, ch('j'), NOW_MS);
        }
        update(&mut state, ch('a'), NOW_MS);
        assert_eq!(
            &shown(&state)[4..],
            [(OPUS.to_owned(), false), (SONNET.to_owned(), false)]
        );
        assert!(update(&mut state, key(KeyCode::Enter), NOW_MS).is_empty());
        let lines = draw(&state, 80, 24);
        assert!(lines.iter().any(|l| l.contains(NO_MODEL)), "{lines:?}");
        update(&mut state, ch('a'), NOW_MS);
        update(&mut state, key(KeyCode::Home), NOW_MS);
        update(&mut state, ch('a'), NOW_MS);
        assert!(update(&mut state, key(KeyCode::Enter), NOW_MS).is_empty());
        let Some(Overlay::Filter(d)) = &state.overlay else {
            panic!("closed");
        };
        assert_eq!(d.error.as_deref(), Some(NO_ACCOUNT));
    }

    #[test]
    fn the_dialog_takes_every_key_and_esc_leaves_the_filter_as_it_was() {
        // R9: `q` and `1` are the dialog's while it is open.
        let mut state = dialog();
        update(&mut state, ch('q'), NOW_MS);
        update(&mut state, ch('1'), NOW_MS);
        assert_eq!((state.quitting, state.screen), (None, Screen::Tokens));
        update(&mut state, ch(' '), NOW_MS);
        assert!(update(&mut state, key(KeyCode::Esc), NOW_MS).is_empty());
        assert!(state.overlay.is_none());
        assert_eq!(state.tokens.query(NOW_MS).accounts, None);
    }

    #[test]
    fn an_inventory_for_a_dialog_closed_or_reopened_since_is_dropped() {
        let mut state = fresh(Screen::Tokens);
        state.start();
        let first = update(&mut state, ch('f'), NOW_MS);
        update(&mut state, key(KeyCode::Esc), NOW_MS);
        let [Cmd::LoadInventory { r#gen, query }] = first.as_slice() else {
            panic!("{first:?}");
        };
        let result = Ok(indexed(query.clone()));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        assert!(state.overlay.is_none(), "closed stays closed");
        update(&mut state, ch('f'), NOW_MS);
        let result = Ok(indexed(query.clone()));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        assert!(
            shown(&state).is_empty(),
            "the reopened dialog waits for its own"
        );
        let failed = Err(SourceError {
            kind: "store".into(),
            message: "the store is locked".into(),
        });
        let r#gen = state.gens.inventory;
        update(
            &mut state,
            Msg::Inventory {
                r#gen,
                result: failed,
            },
            NOW_MS,
        );
        let lines = draw(&state, 80, 24);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("the choices could not be read: the store is locked")),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains(LOADING)),
            "a failed read is not still loading: {lines:?}"
        );
    }

    #[test]
    fn a_long_list_keeps_the_cursor_and_the_keys_in_view_at_80x24() {
        let mut state = fresh(Screen::Tokens);
        state.start();
        let cmds = update(&mut state, ch('f'), NOW_MS);
        let [Cmd::LoadInventory { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let month = PeriodKey::Month {
            year: 2026,
            month: 9,
        };
        let rows = (1..=40)
            .map(|n| {
                let model = format!("model-{n:02}");
                token_row(
                    month.clone(),
                    Some(row_account(1)),
                    Some(&model),
                    counts(1, 1, 1),
                    None,
                )
            })
            .collect();
        let result = Ok(token_report(query.clone(), rows));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        for _ in 0..40 {
            update(&mut state, ch('j'), NOW_MS);
        }
        let lines = draw(&state, 80, 24);
        assert!(
            lines.iter().any(|l| l.contains("[x] model-40")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains(KEYS)), "{lines:?}");
        update(&mut state, key(KeyCode::Home), NOW_MS);
        let lines = draw(&state, 80, 24);
        assert!(lines.iter().any(|l| l.contains("Accounts")), "{lines:?}");
    }

    #[test]
    fn a_long_choice_wraps_under_its_mark_whole_and_the_cursors_shows_whole() {
        // B27: a choice wider than the dialog wraps under its mark, a word wider than a row
        // breaking by width; one that fits `CHOICE_ROWS` shows whole, and the list keeps the
        // cursor's whole choice in view.
        let alias = "a-rather-long-alias-for-the-work-account";
        let label = "averyveryverylongname.with.a.long.tail@example.com";
        let model = "claude-opus-5-5-with-a-very-long-suffix-and-then-some-more-of-it-yet-again";
        let mut state = fresh(Screen::Tokens);
        let mut accounts = many(2);
        accounts[1].row.alias = Some(alias.into());
        accounts[1].row.label = label.into();
        accounts[1].row.email = Some(label.into());
        state.accounts.apply(snapshot(accounts));
        state.start();
        let cmds = update(&mut state, ch('f'), NOW_MS);
        let [Cmd::LoadInventory { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let month = PeriodKey::Month {
            year: 2026,
            month: 9,
        };
        let mut rows: Vec<_> = (1..=12)
            .map(|n| {
                let name = format!("{model}-{n:02}");
                token_row(
                    month.clone(),
                    Some(row_account(1)),
                    Some(&name),
                    counts(1, 1, 1),
                    None,
                )
            })
            .collect();
        rows.push(token_row(
            month,
            Some(row_account(1)),
            Some(OPUS),
            counts(1, 1, 1),
            None,
        ));
        let result = Ok(token_report(query.clone(), rows));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        let squash = |text: &str| {
            text.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        };
        let inside = |lines: &[String]| -> Vec<String> {
            lines
                .iter()
                .filter(|l| l.contains('│'))
                .map(|l| l.split('│').nth(1).unwrap_or("").to_owned())
                .collect()
        };
        let lines = inside(&draw(&state, 80, 24));
        let read = squash(&lines.concat());
        assert!(
            read.contains(&squash(&format!("[x] 2 {alias} ({label})"))),
            "{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains('…')), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains(KEYS)), "{lines:?}");
        update(&mut state, key(KeyCode::End), NOW_MS);
        let lines = inside(&draw(&state, 80, 24));
        assert!(
            squash(&lines.concat()).contains(&squash(&format!("[x] {model}-12"))),
            "the last choice whole: {lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains(KEYS)), "{lines:?}");
        // A long failure wraps too.
        let mut state = fresh(Screen::Tokens);
        state.start();
        update(&mut state, ch('f'), NOW_MS);
        let message = "the store could not be opened because another tagteam holds its lock";
        let failed = Err(SourceError {
            kind: "store".into(),
            message: message.into(),
        });
        let r#gen = state.gens.inventory;
        let msg = Msg::Inventory {
            r#gen,
            result: failed,
        };
        update(&mut state, msg, NOW_MS);
        let lines = inside(&draw(&state, 80, 24));
        let whole = format!("the choices could not be read: {message}");
        assert!(
            squash(&lines.concat()).contains(&squash(&whole)),
            "{lines:?}"
        );
    }

    #[test]
    fn a_choice_takes_three_rows_at_most_and_every_choice_and_the_keys_show() {
        // B29: a 1,400-column model shows its start on three rows, the last cut with `…`. The
        // dialog keeps its own rows for the keys and a failure, so at 80×24 the keys always
        // show, and so does whichever choice the cursor is on.
        let long = format!("claude-{}", "x".repeat(1393));
        let mut state = fresh(Screen::Tokens);
        state.start();
        let cmds = update(&mut state, ch('f'), NOW_MS);
        let [Cmd::LoadInventory { r#gen, query }] = cmds.as_slice() else {
            panic!("{cmds:?}");
        };
        let month = PeriodKey::Month {
            year: 2026,
            month: 9,
        };
        let mut models: Vec<String> = (1..=12).map(|n| format!("claude-test-{n:02}")).collect();
        models.push(long.clone());
        let rows = models
            .iter()
            .map(|m| {
                token_row(
                    month.clone(),
                    Some(row_account(1)),
                    Some(m),
                    counts(1, 1, 1),
                    None,
                )
            })
            .collect();
        let result = Ok(token_report(query.clone(), rows));
        update(
            &mut state,
            Msg::Inventory {
                r#gen: *r#gen,
                result,
            },
            NOW_MS,
        );
        let inside = |lines: &[String]| -> Vec<String> {
            lines
                .iter()
                .filter(|l| l.contains('│'))
                .map(|l| l.split('│').nth(1).unwrap_or("").trim_end().to_owned())
                .collect()
        };
        let lines = inside(&draw(&state, 80, 24));
        assert!(
            lines.iter().any(|l| l.contains("[x] 1 a1@x.co")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains(KEYS)), "{lines:?}");
        // The long model is the last; End puts the cursor on it.
        update(&mut state, key(KeyCode::End), NOW_MS);
        let lines = inside(&draw(&state, 80, 24));
        let at = lines
            .iter()
            .position(|l| l.starts_with(" [x] claude-xxx"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(lines[at + 1].contains(&"x".repeat(40)), "{lines:?}");
        assert!(lines[at + 2].ends_with('…'), "{lines:?}");
        assert!(lines[at + 3].contains(KEYS), "{lines:?}");
        // A long failure takes three rows too, and the dialog's own line still shows.
        let mut state = fresh(Screen::Tokens);
        state.start();
        update(&mut state, ch('f'), NOW_MS);
        let failed = Err(SourceError {
            kind: "store".into(),
            message: "x".repeat(1400),
        });
        let r#gen = state.gens.inventory;
        let msg = Msg::Inventory {
            r#gen,
            result: failed,
        };
        update(&mut state, msg, NOW_MS);
        let lines = inside(&draw(&state, 80, 24));
        let at = lines
            .iter()
            .position(|l| l.contains("the choices could not be read"))
            .unwrap_or_else(|| panic!("{lines:?}"));
        assert!(lines[at + 2].ends_with('…'), "{lines:?}");
        assert!(lines[at + 3].contains("Esc cancel"), "{lines:?}");
    }
}
