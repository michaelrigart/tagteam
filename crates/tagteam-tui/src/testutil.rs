//! Fixtures for the state machine's, the screens' and the overlays' tests: views built from
//! the engine's public types, keys, and a `TestBackend` read back as text.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use jiff::tz::TimeZone;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, CellWidth};
use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProviderId, Sample, Window, WindowKind};
use tagteam_daemon::protocol::{
    Status, StatusDaemon, StatusJob, StatusNotifications, StatusProvider,
};
use tagteam_engine::store::AccountRow;
use tagteam_engine::tokens::{
    Cost, IndexState, PeriodKey, RowAccount, RowKey, TokenCounts, TokenGrouping, TokenPeriod,
    TokenQuery, TokenReport, TokenRow, TokenTotal,
};
use tagteam_engine::views::{
    AccountView, HistoryView, HistoryWindow, ProviderAccounts, ProviderNames, UsageStatus,
    UsageView,
};
use tagteam_provider::KindTraits;

use crate::live::Live;
use crate::msg::{Cmd, Msg};
use crate::screens::tokens::TokensState;
use crate::source::AccountsSnapshot;
use crate::state::{Screen, State};
use crate::theme::Theme;

/// Any fixed instant: every reading, reset and sample below is placed relative to it.
pub(crate) const NOW_S: i64 = 1_790_000_000;
pub(crate) const NOW_MS: i64 = NOW_S * 1000;
pub(crate) const HOUR: i64 = 3_600;

pub(crate) const CC_NAMES: ProviderNames = ProviderNames {
    display_name: "Claude Code",
    launch_command: "claude",
};

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
            identity_json: Default::default(),
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

/// One Claude Code provider holding `accounts`, read at `NOW_S`.
pub(crate) fn snapshot(accounts: Vec<AccountView>) -> AccountsSnapshot {
    AccountsSnapshot {
        providers: vec![ProviderAccounts {
            provider: cc(),
            active_position: accounts.iter().find(|v| v.active).map(|v| v.row.position),
            accounts,
            threshold: 90.0,
            strategy: tagteam_core::autoswitch::Strategy::Best,
            names: CC_NAMES,
        }],
        this: None,
        unmanaged_live: None,
        in_run_shell: false,
        read_at_s: NOW_S,
    }
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
    let mut state = State::new(screen, Theme::new(false), None, tokens, NOW_MS);
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

/// Where `needle` first appears in `buf`: its column and row.
pub(crate) fn locate(buf: &Buffer, needle: &str) -> Option<(u16, u16)> {
    (0..buf.area.height).find_map(|y| {
        let (text, starts) = row(buf, y);
        let at = text.find(needle)?;
        starts.iter().find(|(b, _)| *b == at).map(|(_, x)| (*x, y))
    })
}

/// A sample of one window taken `ago_s` before `NOW_S`.
pub(crate) fn sample(ago_s: i64, pct: f64) -> Sample {
    Sample {
        fetched_at: NOW_S - ago_s,
        pct,
        resets_at: None,
    }
}

pub(crate) fn history_window(window: Window, samples: Vec<Sample>, pace: Pace) -> HistoryWindow {
    HistoryWindow {
        window,
        samples,
        pace,
    }
}

pub(crate) fn history_of(account: AccountView, windows: Vec<HistoryWindow>) -> HistoryView {
    HistoryView {
        account,
        windows,
        unmatched_window: false,
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
/// counting `counts`, costing `cost` nano-dollars (or nothing), keyed as the report keys it.
pub(crate) fn token_row(
    period: PeriodKey,
    account: Option<RowAccount>,
    model: Option<&str>,
    counts: TokenCounts,
    cost: Option<u64>,
) -> TokenRow {
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
        mode: mode.into(),
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
