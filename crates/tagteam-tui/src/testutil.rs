//! Fixtures for the state machine's, the screens' and the overlays' tests: views built from
//! the engine's public types, keys, and a `TestBackend` read back as text.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use jiff::tz::TimeZone;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell, CellWidth};
use ratatui::layout::Rect;
use ratatui::style::Style;
use tagteam_cc::CLAUDE_CODE;
use tagteam_core::tokens::{day_bounds_ms, month_bounds_ms};
use tagteam_core::{AccountId, Pace, ProviderId, Window, WindowKind};
use tagteam_daemon::protocol::{
    Status, StatusDaemon, StatusJob, StatusNotifications, StatusProvider, Wire,
};
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::LiveLogin;
use tagteam_engine::timeline::{Lane, Series, TimelineView};
use tagteam_engine::tokens::{
    Cost, IndexState, PeriodKey, RowAccount, RowKey, TokenCounts, TokenGrouping, TokenPeriod,
    TokenQuery, TokenReport, TokenRow, TokenTotal,
};
use tagteam_engine::views::{
    AccountView, ProviderAccounts, ProviderNames, UsageStatus, UsageView, forecast_of,
};
use tagteam_provider::KindTraits;

use crate::live::Live;
use crate::msg::{Cmd, Msg};
use crate::screens::tokens::TokensState;
use crate::source::AccountsSnapshot;
use crate::state::{Screen, Startup, State};
use crate::theme::{Background, Role, Theme};

/// Any fixed instant: every reading, reset and sample below is placed relative to it.
pub(crate) const NOW_S: i64 = 1_790_000_000;
pub(crate) const NOW_MS: i64 = NOW_S * 1000;
pub(crate) const HOUR: i64 = 3_600;

pub(crate) const CC_NAMES: ProviderNames = ProviderNames {
    display_name: "Claude Code",
    launch_command: "claude",
};

/// Tiers 0, 1 and 2 (dark, then light), as the redesign's tests go through them.
pub(crate) const TIERS: [Theme; 4] = [
    Theme::new(false),
    Theme::new(true),
    Theme::new(true).truecolor(Background::Dark),
    Theme::new(true).truecolor(Background::Light),
];

pub(crate) const OAUTH: KindTraits = KindTraits {
    refreshable: true,
    managed_key_axis: false,
    default_email_prefix: None,
    display: None,
};

pub(crate) const API_KEY: KindTraits = KindTraits {
    refreshable: false,
    managed_key_axis: true,
    default_email_prefix: Some("api-key"),
    display: Some("api key"),
};

pub(crate) fn cc() -> ProviderId {
    ProviderId::new(CLAUDE_CODE)
}

/// The id every fixture account at `position` has.
pub(crate) fn id(position: u32) -> AccountId {
    AccountId::from_string(format!("id-{position}"))
}

/// `email`'s inactive Claude Code account at `position`, with `usage`.
pub(crate) fn account(position: u32, email: &str, usage: UsageView) -> AccountView {
    AccountView {
        row: AccountRow {
            id: id(position),
            provider: cc(),
            position,
            identity_key: format!("{email}\n"),
            label: email.into(),
            email: Some(email.into()),
            org_uuid: String::new(),
            org_name: None,
            account_uuid: None,
            kind: "oauth".into(),
            alias: None,
            disabled: false,
            identity_json: serde_json::json!({}),
            login_expires_at: None,
            login_epoch: 0,
            replacing_fp: None,
            quarantine_reason: None,
            quarantine_fp: None,
            quarantine_at: None,
            added_at: 1,
        },
        active: false,
        kind: OAUTH,
        usage,
        in_session: false,
    }
}

/// No reading: `status`, with `error` and a retry `retry_in` seconds from now.
pub(crate) fn unread(status: UsageStatus, error: Option<&str>, retry_in: Option<i64>) -> UsageView {
    UsageView {
        status,
        windows: None,
        decision_grade: false,
        fetched_at: None,
        age_s: None,
        error: error.map(str::to_owned),
        retry_at: retry_in.map(|s| NOW_S + s),
        next_poll_at: None,
    }
}

pub(crate) fn window(
    key: &str,
    label: &str,
    kind: WindowKind,
    pct: f64,
    resets_in: Option<i64>,
) -> Window {
    Window {
        key: key.into(),
        label: label.into(),
        kind,
        pct,
        resets_at: resets_in.map(|s| NOW_S + s),
        period_s: None,
        detail: None,
    }
}

/// A good reading `age_s` old: 5h at `five` resetting in 2h40m30s, 7d at `seven` resetting in
/// 3d09h00m30s and ahead of pace when `ahead`.
pub(crate) fn read(age_s: i64, five: f64, seven: f64, ahead: bool) -> UsageView {
    UsageView {
        status: UsageStatus::Ok,
        windows: Some(vec![
            (
                window("5h", "5h", WindowKind::Short, five, Some(9_630)),
                Pace::default(),
            ),
            (
                window("7d", "7d", WindowKind::Long, seven, Some(291_630)),
                Pace {
                    ahead: Some(ahead),
                    ..Pace::default()
                },
            ),
        ]),
        decision_grade: age_s <= 300,
        fetched_at: Some(NOW_S - age_s),
        age_s: Some(age_s),
        error: None,
        retry_at: None,
        next_poll_at: None,
    }
}

/// One Claude Code provider holding `accounts`, read at `NOW_S`: its live login is its active
/// account's, else none.
pub(crate) fn snapshot(accounts: Vec<AccountView>) -> AccountsSnapshot {
    let active_position = accounts.iter().find(|v| v.active).map(|v| v.row.position);
    let live = active_position.map_or(LiveLogin::None, |position| LiveLogin::Managed { position });
    AccountsSnapshot {
        providers: vec![ProviderAccounts {
            provider: cc(),
            active_position,
            forecast: forecast_of(&accounts, &live, &[]),
            accounts,
            threshold: 90.0,
            strategy: tagteam_core::autoswitch::AutoStrategy::Best,
            names: CC_NAMES,
            models: vec![],
            live,
        }],
        this: None,
        in_run_shell: false,
        read_at_s: NOW_S,
        auto_holders: vec![],
        config_mtime: None,
        last_auto: vec![],
    }
}

/// Sets `view`'s live login to `live`, and its forecast to the one the engine computes with it.
pub(crate) fn set_live(view: &mut ProviderAccounts, live: LiveLogin) {
    view.forecast = forecast_of(&view.accounts, &live, &view.models);
    view.live = live;
}

/// `n` accounts `a1@x.co` … at positions 1 to `n`, each with a reading.
pub(crate) fn many(n: u32) -> Vec<AccountView> {
    (1..=n)
        .map(|p| account(p, &format!("a{p}@x.co"), read(60, 10.0, 20.0, false)))
        .collect()
}

/// A State on `screen` at 80×24, without colour, nothing loaded yet. Tokens reports on Claude
/// Code in UTC.
pub(crate) fn fresh(screen: Screen) -> State {
    let tokens = TokensState::new(cc(), TimeZone::UTC);
    let mut state = State::new(
        screen,
        Theme::new(false),
        None,
        tokens,
        CC_NAMES,
        Startup::default(),
        NOW_MS,
    );
    state.size = (80, 24);
    state
}

pub(crate) fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(crate) fn key(code: KeyCode) -> Msg {
    Msg::Key(press(code))
}

pub(crate) fn ch(c: char) -> Msg {
    let modifiers = if c.is_ascii_uppercase() {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    Msg::Key(KeyEvent::new(KeyCode::Char(c), modifiers))
}

pub(crate) fn ctrl(c: char) -> Msg {
    Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

pub(crate) fn terminal(w: u16, h: u16) -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(w, h)).unwrap()
}

/// What `view` draws of `state` on a `w`×`h` terminal at `now_ms`.
pub(crate) fn draw_buffer(state: &State, w: u16, h: u16, now_ms: i64) -> Buffer {
    let mut terminal = terminal(w, h);
    terminal
        .draw(|frame| crate::view::view(state, frame, now_ms))
        .unwrap();
    terminal.backend().buffer().clone()
}

pub(crate) fn draw(state: &State, w: u16, h: u16) -> Vec<String> {
    draw_at(state, w, h, NOW_MS)
}

pub(crate) fn draw_at(state: &State, w: u16, h: u16, now_ms: i64) -> Vec<String> {
    text_of(&draw_buffer(state, w, h, now_ms))
}

/// What `view` draws of `state` in the body's rows alone (`view::areas`), at `w`×`h` and
/// `now_ms`, trailing spaces trimmed.
pub(crate) fn body_of(state: &State, w: u16, h: u16, now_ms: i64) -> Vec<String> {
    let body = crate::view::areas(state, ratatui::layout::Rect::new(0, 0, w, h)).body;
    let mut rows = draw_at(state, w, h, now_ms);
    rows.truncate(usize::from(body.y + body.height));
    rows.split_off(usize::from(body.y))
}

/// What a panel shows of its text on a `w`×`h` terminal: the rows of the body between the
/// panel's border and padding (four rows less than the body), each without the border and
/// padding beside it, trailing spaces trimmed.
pub(crate) fn panel_text(state: &State, w: u16, h: u16) -> Vec<String> {
    let body = crate::view::areas(state, Rect::new(0, 0, w, h)).body;
    let mut rows = draw(state, w, h);
    rows.truncate(usize::from(body.y + body.height).saturating_sub(2));
    rows.split_off(usize::from(body.y) + 2)
        .iter()
        .map(|l| {
            l.chars()
                .skip(3)
                .take(usize::from(w) - 6)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// Each row of `buf` as text, trailing spaces trimmed. A cell a wide character covers is
/// skipped, as a terminal shows it.
pub(crate) fn text_of(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| row(buf, y).0.trim_end().to_owned())
        .collect()
}

/// Row `y` as text, with the column each byte of it starts at.
fn row(buf: &Buffer, y: u16) -> (String, Vec<(usize, u16)>) {
    let mut text = String::new();
    let mut starts = Vec::new();
    let mut hidden = 0u16;
    for x in 0..buf.area.width {
        let cell = &buf[(x, y)];
        if hidden == 0 {
            starts.push((text.len(), x));
            text.push_str(cell.symbol());
        }
        hidden = hidden.max(cell.cell_width()).saturating_sub(1);
    }
    (text, starts)
}

/// The letters `roles` reads, in the order it tries them.
const LETTERS: [(char, Role); 6] = [
    ('c', Role::Crit),
    ('w', Role::Warn),
    ('o', Role::Ok),
    ('a', Role::Accent),
    ('f', Role::Faint),
    ('m', Role::Muted),
];

/// Row `y` of `buf` as role letters: `c w o a f m` (crit, warn, ok, accent, faint, muted), the first
/// whose non-empty style the cell's contains (fg equal where the role sets one, its modifiers
/// present), else `t`; a space cell is ` `. A cell a wide character covers is skipped, as
/// `text_of` skips it.
pub(crate) fn roles(buf: &Buffer, theme: Theme, y: u16) -> String {
    let mut letters = String::new();
    let mut hidden = 0u16;
    for x in 0..buf.area.width {
        let cell = &buf[(x, y)];
        if hidden == 0 {
            letters.push(letter(cell, theme));
        }
        hidden = hidden.max(cell.cell_width()).saturating_sub(1);
    }
    letters
}

fn letter(cell: &Cell, theme: Theme) -> char {
    if cell.symbol() == " " {
        return ' ';
    }
    let shows = |style: Style| {
        style != Style::new()
            && style.fg.is_none_or(|fg| cell.fg == fg)
            && cell.modifier.contains(style.add_modifier)
    };
    LETTERS
        .into_iter()
        .find(|(_, role)| shows(theme.role(*role)))
        .map_or('t', |(letter, _)| letter)
}

/// An expected row in the same letters, collapsed as `theme` draws them (tier 0 shares
/// styles): each letter's role drawn in one cell and read back by `roles`; `B` is bold text.
pub(crate) fn canon(theme: Theme, row: &str) -> String {
    row.chars()
        .map(|c| {
            let style = match c {
                ' ' => return ' ',
                't' => Style::new(),
                'B' => theme.strong(),
                _ => {
                    let (_, role) = LETTERS
                        .into_iter()
                        .find(|(letter, _)| *letter == c)
                        .unwrap_or_else(|| panic!("{c:?} is no role letter"));
                    theme.role(role)
                }
            };
            let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
            buf[(0, 0)].set_symbol("x").set_style(style);
            letter(&buf[(0, 0)], theme)
        })
        .collect()
}

/// The footer row of keys `left` on a terminal `width` wide, as `text_of` reads it: `left` from
/// column 0, a gap, then `? keys  q quit` ending one column in.
pub(crate) fn footer_row(left: &str, width: usize) -> String {
    let pinned = "? keys  q quit";
    let gap = width - 1 - crate::view::width_of(left) - crate::view::width_of(pinned);
    format!("{left}{}{pinned}", " ".repeat(gap))
}

/// Where `needle` first appears in `buf`: its column and row.
pub(crate) fn locate(buf: &Buffer, needle: &str) -> Option<(u16, u16)> {
    (0..buf.area.height).find_map(|y| {
        let (text, starts) = row(buf, y);
        let at = text.find(needle)?;
        starts.iter().find(|(b, _)| *b == at).map(|(_, x)| (*x, y))
    })
}

/// A view over `columns` columns of the 7 days to NOW_S, threshold 90.
pub(crate) fn timeline_view(columns: usize, charts: Vec<Series>, lanes: Vec<Lane>) -> TimelineView {
    TimelineView {
        since_s: NOW_S - 7 * 86_400,
        now_s: NOW_S,
        threshold: 90.0,
        columns,
        charts,
        lanes,
        events: vec![],
        paused_at_start: false,
    }
}

/// `many()`'s account `position` as a lane.
pub(crate) fn lane_of(position: u32, peaks: Vec<Option<f64>>, in_use: Vec<bool>) -> Lane {
    Lane {
        account: tagteam_engine::timeline::AccountRef {
            id: id(position),
            position: Some(position),
            alias: None,
            email: Some(format!("a{position}@x.co")),
            removed: false,
        },
        peaks,
        in_use,
    }
}

/// Fixture account `position` as a report row names it.
pub(crate) fn row_account(position: u32) -> RowAccount {
    RowAccount::Account {
        id: id(position),
        position,
        label: format!("a{position}@x.co"),
        alias: None,
    }
}

/// The day `days_ago` days before `NOW_MS`'s, in UTC.
pub(crate) fn day(days_ago: i64) -> PeriodKey {
    let at = jiff::Timestamp::from_millisecond(NOW_MS - days_ago * 24 * HOUR * 1000).unwrap();
    PeriodKey::Day {
        date: at.to_zoned(TimeZone::UTC).date(),
    }
}

/// `input` and `output` tokens, and `cache_read` read from the cache.
pub(crate) fn counts(input: u64, output: u64, cache_read: u64) -> TokenCounts {
    TokenCounts {
        input,
        output,
        cache_read,
        ..TokenCounts::default()
    }
}

/// A row of `period`, grouped by `account` and `model` when they are given, of one response
/// counting `counts`, costing `cost` nano-dollars (or nothing), keyed as the report keys it, at
/// the latest its period allows.
pub(crate) fn token_row(
    period: PeriodKey,
    account: Option<RowAccount>,
    model: Option<&str>,
    counts: TokenCounts,
    cost: Option<u64>,
) -> TokenRow {
    // In UTC, as `token_query`'s.
    let last_ms = match &period {
        PeriodKey::Day { date } => day_bounds_ms(*date, &TimeZone::UTC).1 - 1,
        PeriodKey::Month { year, month } => month_bounds_ms(*year, *month, &TimeZone::UTC).1 - 1,
        PeriodKey::Session { last_ms, .. } => *last_ms,
        PeriodKey::Block { end_ms, .. } => end_ms - 1,
    };
    TokenRow {
        key: RowKey::new(&period, account.as_ref(), model),
        accounts: account.iter().cloned().collect(),
        models: model.iter().map(|m| (*m).to_owned()).collect(),
        period,
        account,
        model: model.map(str::to_owned),
        counts,
        responses: 1,
        unfinalized: 0,
        cost: cost.map(|usd_nanos| Cost {
            usd_nanos,
            partial: false,
        }),
        last_ms,
    }
}

/// Claude Code's report by `period`, grouped by account, over the 30 days to `NOW_MS`, in UTC.
pub(crate) fn token_query(period: TokenPeriod) -> TokenQuery {
    TokenQuery {
        provider: cc(),
        period,
        grouping: TokenGrouping::Account,
        since_ms: Some(NOW_MS - 30 * 24 * HOUR * 1000),
        until_ms: None,
        accounts: None,
        models: None,
        session: None,
        tz: TimeZone::UTC,
    }
}

/// The report `query` asked for, holding `rows`: its total is theirs, and its index was
/// refreshed and last changed a minute before `NOW_MS`.
pub(crate) fn token_report(query: TokenQuery, rows: Vec<TokenRow>) -> TokenReport {
    let mut total = TokenTotal::default();
    for row in &rows {
        let (t, c) = (&mut total.counts, &row.counts);
        t.input += c.input;
        t.output += c.output;
        t.cache_write_5m += c.cache_write_5m;
        t.cache_write_1h += c.cache_write_1h;
        t.cache_read += c.cache_read;
        total.responses += row.responses;
        total.unfinalized += row.unfinalized;
        total.cost = match (total.cost, row.cost) {
            (None, cost) | (cost, None) => cost,
            (Some(a), Some(b)) => Some(Cost {
                usd_nanos: a.usd_nanos + b.usd_nanos,
                partial: a.partial || b.partial,
            }),
        };
    }
    TokenReport {
        query,
        time_zone: "UTC".into(),
        rows,
        total,
        index: IndexState {
            refreshed_at_ms: Some(NOW_MS - 60_000),
            changed_at_ms: Some(NOW_MS - 60_000),
            files: 3,
            responses: 3,
            ..IndexState::default()
        },
        unpriced_models: vec![],
        prices_as_of: "2026-10-01",
        price_overrides: vec![],
        tracking_since_ms: None,
    }
}

/// The fixture daemon's `status` (D5.4): pid 4242, notifications off, with `providers` and
/// `jobs`.
pub(crate) fn daemon_status(providers: Vec<StatusProvider>, jobs: Vec<StatusJob>) -> Status {
    Status {
        daemon: StatusDaemon {
            pid: 4242,
            version: "20261014.2045.0".into(),
            protocol: 1,
            started_at: "2026-09-21T14:13:20Z".into(),
            socket: Some("~/.local/share/tagteam/daemon.sock".into()),
            log: "~/.local/state/tagteam/tagteam.log".into(),
        },
        notifications: StatusNotifications {
            enabled: false,
            backend: None,
            events: vec![],
        },
        providers,
        jobs,
        subscribers: 1,
    }
}

/// `provider`'s job in `mode` (D4.3), with `autoswitch`, waiting for nothing, with no backoff.
pub(crate) fn daemon_job(provider: &str, mode: &str, autoswitch: bool) -> StatusProvider {
    StatusProvider {
        provider: provider.into(),
        mode: Wire::from_wire(mode),
        autoswitch,
        waiting_for: None,
        holder_pid: None,
        backoff: None,
        last_tick_at: None,
        next_tick_at: None,
        last_outcome: None,
        last_switch: None,
    }
}

/// Links `state` to the daemon at `NOW_MS`, by a link of its own, and answers the `status` it
/// asks for: live mode. The re-read the link asks for is still to come. Returns the link.
pub(crate) fn go_live(state: &mut State, status: Status) -> u64 {
    static LINKS: AtomicU64 = AtomicU64::new(100);
    let link = LINKS.fetch_add(1, Ordering::Relaxed);
    let cmds = crate::update::update(state, Msg::Live(Live::Up { link }), NOW_MS);
    assert_eq!(cmds, [Cmd::FetchStatus { link }]);
    let result = Ok(status);
    crate::update::update(state, Msg::DaemonStatus { link, result }, NOW_MS);
    assert!(state.daemon.live());
    link
}
