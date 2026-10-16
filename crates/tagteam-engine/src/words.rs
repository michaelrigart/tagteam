//! The words `list`, `status`, `history` and `tokens` print, shared with the TUI so that both
//! say the same things (TUI spec §5.2, §5.3). They moved here from the CLI's `render.rs`,
//! `history.rs` and `tokens_cmd.rs` with their bodies unchanged; the CLI adds its ANSI colours
//! around them.

use tagteam_core::{Window, WindowKind};

use crate::store::AccountRow;
use crate::tokens::TokenReport;
use crate::views::{AccountView, HistoryWindow, NO_DATA, UsageStatus, UsageView};

/// A window a reading does not have, or an age that is not known.
pub const MISSING: &str = "—";
/// §8.7's marker for a window ahead of pace.
pub const AHEAD: &str = "▲ pace";
/// §13.1's marker of a session-owned account, in a one-column field after the position
/// (Decision 11).
pub const IN_SESSION: &str = "▶";
/// The trailing note on the run shell's own row: §13.1's `▶ this`, kept with the notes so the
/// columns stay aligned.
pub const THIS: &str = "this";
/// `list` always has a spend column once any account has a reading (§13.1).
pub const SPEND_HEAD: &str = "SPEND";
/// A projection further out than this is not worth a countdown: it will not run out.
const A_YEAR_S: i64 = 365 * 86_400;

/// The account's email (or label when it has none).
pub fn email(r: &AccountRow) -> String {
    r.email.clone().unwrap_or_else(|| r.label.clone())
}

pub fn name(r: &AccountRow) -> String {
    let email = email(r);
    match &r.alias {
        Some(a) => format!("{a} ({email})"),
        None => email,
    }
}

/// `name`, and the organisation in brackets when the login has one: `list`'s ACCOUNT cell.
pub fn account_cell(r: &AccountRow) -> String {
    match &r.org_name {
        Some(org) => format!("{} [{org}]", name(r)),
        None => name(r),
    }
}

// A span of time as `list` and `status` show it: `3d09h`, `2h40m`, `45m`, or `<1m`. The usage
// strategies' messages state a reset in the same words, so there is one formatter.
pub use tagteam_core::rank::span as duration;

/// The time left until `at`, or `reset` once it has passed: the reading predates the window's
/// reset, so its pct no longer applies.
pub fn countdown(at: i64, now_s: i64) -> String {
    if at <= now_s {
        "reset".into()
    } else {
        duration(at - now_s)
    }
}

/// §13.5's severities, which `list`, `status` and the TUI share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Normal,
    Warning,
    Critical,
}

/// ≥ 90 critical, ≥ 70 warning.
pub fn severity(pct: i64) -> Severity {
    if pct >= 90 {
        Severity::Critical
    } else if pct >= 70 {
        Severity::Warning
    } else {
        Severity::Normal
    }
}

/// An amount with its currency's symbol (€, $, £), or its code for any other; a whole amount
/// without cents.
pub fn money(amount: f64, currency: &str) -> String {
    let cents = format!("{amount:.2}");
    let n = match cents.strip_suffix(".00") {
        Some("-0") => "0",
        Some(whole) => whole,
        None => &cents,
    }
    .to_owned();
    match currency.to_ascii_uppercase().as_str() {
        "EUR" => format!("€{n}"),
        "USD" => format!("${n}"),
        "GBP" => format!("£{n}"),
        code => format!("{n} {code}"),
    }
}

/// `€0 of €20`, from a spend window's `detail {used, limit, currency}` (§8.2).
pub fn spend_text(w: &Window) -> Option<String> {
    let d = w.detail.as_ref().filter(|_| w.kind == WindowKind::Spend)?;
    let currency = d["currency"].as_str()?;
    Some(format!(
        "{} of {}",
        money(d["used"].as_f64()?, currency),
        money(d["limit"].as_f64()?, currency)
    ))
}

/// A usage status in words (§13.1): in place of a row's windows when it has no reading, and
/// beside them when its status is not `ok`. Whatever the status, a retry still ahead is said:
/// `keychain unavailable (retry 2m)`, `unavailable (http-429, retry 5m)`.
pub fn usage_words(u: &UsageView, now_s: i64) -> String {
    let (head, why) = match u.status {
        UsageStatus::Ok => ("no usage reported", None),
        UsageStatus::TokenExpired => ("token expired", None),
        UsageStatus::ApiKey => ("api key", None),
        UsageStatus::KeychainUnavailable => ("keychain unavailable", None),
        UsageStatus::ReloginRequired => ("relogin required", None),
        UsageStatus::ForeignCredential => ("foreign credential", None),
        UsageStatus::NoCredentials => ("no credentials", None),
        UsageStatus::Unsupported => ("usage unsupported", None),
        UsageStatus::Unavailable => match u.error.as_deref() {
            None | Some(NO_DATA) => ("no data yet", None),
            Some("over-budget") => ("over budget", None),
            Some(e) => ("unavailable", Some(e.to_owned())),
        },
    };
    let retry = u
        .retry_at
        .filter(|&at| at > now_s)
        .map(|at| format!("retry {}", duration(at - now_s)));
    let detail: Vec<String> = why.into_iter().chain(retry).collect();
    if detail.is_empty() {
        head.to_owned()
    } else {
        format!("{head} ({})", detail.join(", "))
    }
}

/// One of `list`'s window columns: the window key it shows (`None` for the spend column when
/// no account has spend), headed by the window's label in capitals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowColumn {
    pub key: Option<String>,
    pub kind: WindowKind,
    pub head: String,
}

/// §13.1's window columns for one provider's accounts: every window key some reading has, in
/// kind order (Short, Long, Spend, Scoped) and then by first appearance. SPEND is always a
/// column, `—` throughout when no account has spend: only scoped windows come and go. No
/// columns at all while no account has a reading.
pub fn window_columns(accounts: &[AccountView]) -> Vec<WindowColumn> {
    let windows: Vec<&Window> = accounts
        .iter()
        .filter_map(|v| v.usage.windows.as_ref())
        .flatten()
        .map(|(w, _)| w)
        .collect();
    let mut cols: Vec<WindowColumn> = Vec::new();
    if windows.is_empty() {
        return cols;
    }
    for kind in [
        WindowKind::Short,
        WindowKind::Long,
        WindowKind::Spend,
        WindowKind::Scoped,
    ] {
        for w in windows.iter().filter(|w| w.kind == kind) {
            if !cols
                .iter()
                .any(|c| c.key.as_deref() == Some(w.key.as_str()))
            {
                cols.push(WindowColumn {
                    key: Some(w.key.clone()),
                    kind,
                    head: w.label.to_uppercase(),
                });
            }
        }
        if kind == WindowKind::Spend && !windows.iter().any(|w| w.kind == WindowKind::Spend) {
            cols.push(WindowColumn {
                key: None,
                kind,
                head: SPEND_HEAD.into(),
            });
        }
    }
    cols
}

/// The notes that follow a row's cells: its status in words (in place of the windows when it
/// has no reading, beside them when the status is not `ok`), its kind, `disabled`, and `this`
/// when the row is the run shell's own account (§13.1).
pub fn row_notes(v: &AccountView, this: bool, now_s: i64) -> Vec<String> {
    let u = &v.usage;
    let mut notes = Vec::new();
    let has_windows = u.windows.as_deref().is_some_and(|ws| !ws.is_empty());
    if !has_windows || u.status != UsageStatus::Ok {
        notes.push(usage_words(u, now_s));
    }
    if let Some(kind) = v.kind.display {
        if !notes.iter().any(|n| n == kind) {
            notes.push(kind.to_owned());
        }
    }
    if v.row.disabled {
        notes.push("disabled".into());
    }
    if this {
        notes.push(THIS.into());
    }
    notes
}

/// A rate in points per hour with its sign: one decimal, or two below 0.05 in magnitude, where
/// one would print a quiet window's rate as zero (`+0.03`, not `+0.0`). A rate too small for
/// two decimals is `<0.01`, never a zero beside a finite runway; only exactly 0 is `+0.0`.
pub fn rate_signed(rate: f64) -> String {
    let magnitude = rate.abs();
    if magnitude == 0.0 {
        "+0.0".to_owned()
    } else if magnitude < 0.005 {
        format!("{}<0.01", if rate < 0.0 { '-' } else { '+' })
    } else if magnitude < 0.05 {
        format!("{rate:+.2}")
    } else {
        format!("{rate:+.1}")
    }
}

/// The burn rate and where it leads (§8.7): `lasts to reset` when the window will, else when it
/// runs out (or `should have run out` when that time has passed, `won't run out within a year`
/// when it is further off than that), with the method that measured the rate.
pub fn projection_text(hw: &HistoryWindow, now_s: i64) -> String {
    let p = &hw.pace;
    let method = p
        .method
        .map_or_else(String::new, |m| format!(" ({})", m.as_str()));
    // An exhausted window ran out when it was read, which may be long ago: no countdown applies.
    if hw.window.pct >= 100.0 {
        return match p.rate_per_hour {
            Some(rate) => format!("{} pts/h · at the limit{method}", rate_signed(rate)),
            None => "at the limit".to_owned(),
        };
    }
    let Some(rate) = p.rate_per_hour else {
        return "no rate yet".to_owned();
    };
    let eta = match (p.will_last_to_reset, p.exhaustion_at) {
        (Some(true), _) => "lasts to reset".to_owned(),
        (_, Some(at)) if at <= now_s => format!(
            "should have run out {} ago",
            duration(now_s.saturating_sub(at))
        ),
        (_, Some(at)) if at.saturating_sub(now_s) > A_YEAR_S => {
            "won't run out within a year".to_owned()
        }
        (_, Some(at)) => format!("runs out in {}", duration(at - now_s)),
        _ => "no projection".to_owned(),
    };
    format!("{} pts/h · {eta}{method}", rate_signed(rate))
}

/// What a token report with no rows says (token report §11.2).
pub const NO_TOKEN_USAGE: &str = "No token usage in this range.";

/// `1 noun`, or `n nouns`.
pub fn plural(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// A token count as the report shows it (token report §11.2): as is below 1,000, else to one
/// decimal in thousands, millions, billions or trillions (`12.4M`), rounded half up.
pub fn scaled(n: u64) -> String {
    const UNITS: [(u128, char); 4] = [
        (1_000, 'K'),
        (1_000_000, 'M'),
        (1_000_000_000, 'B'),
        (1_000_000_000_000, 'T'),
    ];
    if n < 1_000 {
        return n.to_string();
    }
    let tenths = |unit: u128| (u128::from(n) * 10 + unit / 2) / unit;
    let (unit, suffix) = UNITS
        .iter()
        .copied()
        .find(|&(unit, _)| tenths(unit) < 10_000)
        .unwrap_or(UNITS[3]);
    let t = tenths(unit);
    format!("{}.{}{suffix}", t / 10, t % 10)
}

/// Nano-dollars as dollars and cents (`$312.40`), rounded half up.
pub fn dollars(nanos: u64) -> String {
    let cents = (u128::from(nanos) + 5_000_000) / 10_000_000;
    format!("${}.{:02}", cents / 100, cents % 100)
}

/// Transcript files the token index's last refresh could not read (token report §11.3).
pub fn unreadable_transcripts(n: u64) -> String {
    format!(
        "{} could not be read; the next refresh tries again",
        plural(n, "transcript file")
    )
}

/// The token report's footnotes (token report §11.2), one sentence each: unpriced models,
/// responses whose output is a lower bound (token report §2.3), skipped lines, the transcript
/// files the last refresh could not read when `unreadable` asks for them, and the price
/// table's date. `mark` is what marks a partial cost: `*` in the CLI, `≥` in the TUI (TUI spec
/// §5.3).
pub fn token_footnotes(report: &TokenReport, mark: &str, unreadable: bool) -> Vec<String> {
    let mut notes = Vec::new();
    if !report.unpriced_models.is_empty() {
        notes.push(format!(
            "No price for {}: counted without cost ({mark} marks a partial cost).",
            report.unpriced_models.join(", ")
        ));
    }
    if report.total.unfinalized > 0 {
        notes.push(format!(
            "Output is a lower bound for {}.",
            plural(report.total.unfinalized, "subagent response")
        ));
    }
    if report.index.skipped_lines > 0 {
        notes.push(format!(
            "{} could not be read, so these totals may be low.",
            plural(report.index.skipped_lines, "transcript line")
        ));
    }
    if unreadable && report.index.unreadable_files > 0 {
        notes.push(format!(
            "{}.",
            unreadable_transcripts(report.index.unreadable_files)
        ));
    }
    notes.push(if report.price_overrides.is_empty() {
        format!("API cost at list prices as of {}.", report.prices_as_of)
    } else {
        format!(
            "API cost at list prices as of {}, with config.toml's for {}.",
            report.prices_as_of,
            report.price_overrides.join(", ")
        )
    });
    notes
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProjectionMethod, ProviderId};
    use tagteam_provider::KindTraits;

    use super::*;

    /// Any fixed instant, in seconds: every reading and reset below is placed relative to it.
    const NOW: i64 = 1_790_000_000;
    const HOUR: i64 = 3_600;

    const OAUTH: KindTraits = KindTraits {
        refreshable: true,
        managed_key_axis: false,
        default_email_prefix: None,
        display: None,
    };

    const SETUP_TOKEN: KindTraits = KindTraits {
        refreshable: false,
        managed_key_axis: false,
        default_email_prefix: Some("setup-token"),
        display: Some("setup token"),
    };

    fn row(position: u32, email: &str) -> AccountRow {
        AccountRow {
            id: AccountId::from_string(format!("id-{position}")),
            provider: ProviderId::new(CLAUDE_CODE),
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
            identity_json: json!({}),
            login_expires_at: None,
            login_epoch: 0,
            replacing_fp: None,
            quarantine_reason: None,
            quarantine_fp: None,
            quarantine_at: None,
            added_at: 1,
        }
    }

    fn view(position: u32, email: &str, kind: KindTraits, usage: UsageView) -> AccountView {
        AccountView {
            row: row(position, email),
            active: false,
            kind,
            usage,
            in_session: false,
        }
    }

    /// No reading: `status`, with `error` and a retry `retry_in` seconds from now.
    fn unread(status: UsageStatus, error: Option<&str>, retry_in: Option<i64>) -> UsageView {
        UsageView {
            status,
            windows: None,
            decision_grade: false,
            fetched_at: None,
            age_s: None,
            error: error.map(str::to_owned),
            retry_at: retry_in.map(|s| NOW + s),
            next_poll_at: None,
        }
    }

    fn window(
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
            resets_at: resets_in.map(|s| NOW + s),
            period_s: None,
            detail: None,
        }
    }

    fn spend(used: f64, limit: f64, currency: &str) -> Window {
        Window {
            detail: Some(json!({"used": used, "limit": limit, "currency": currency})),
            ..window(
                "spend",
                "spend",
                WindowKind::Spend,
                used / limit * 100.0,
                None,
            )
        }
    }

    /// A good reading of `windows`, none of them ahead of pace.
    fn read(windows: Vec<Window>) -> UsageView {
        UsageView {
            status: UsageStatus::Ok,
            windows: Some(windows.into_iter().map(|w| (w, Pace::default())).collect()),
            decision_grade: true,
            fetched_at: Some(NOW - 120),
            age_s: Some(120),
            error: None,
            retry_at: None,
            next_poll_at: None,
        }
    }

    fn five_hour() -> Window {
        window("5h", "5h", WindowKind::Short, 9.0, Some(9_630))
    }

    fn seven_day() -> Window {
        window("7d", "7d", WindowKind::Long, 77.0, Some(291_630))
    }

    fn fable() -> Window {
        window(
            "scoped:Fable",
            "Fable",
            WindowKind::Scoped,
            0.0,
            Some(291_630),
        )
    }

    fn history_window(window: Window, pace: Pace) -> HistoryWindow {
        HistoryWindow {
            window,
            samples: vec![],
            pace,
        }
    }

    #[test]
    fn the_names_of_a_row_are_its_alias_email_label_and_organisation() {
        let mut r = row(1, "a@x.co");
        assert_eq!(
            (email(&r), name(&r), account_cell(&r)),
            ("a@x.co".into(), "a@x.co".into(), "a@x.co".into())
        );
        r.alias = Some("work".into());
        assert_eq!(name(&r), "work (a@x.co)");
        r.org_name = Some("Acme".into());
        assert_eq!(account_cell(&r), "work (a@x.co) [Acme]");
        // A token account has no email: its label stands in.
        r.email = None;
        r.label = "api-key-4@token.local".into();
        assert_eq!(email(&r), "api-key-4@token.local");
        assert_eq!(name(&r), "work (api-key-4@token.local)");
    }

    #[test]
    fn durations_read_as_days_hours_or_minutes() {
        for (secs, text) in [
            (-5, "<1m"),
            (0, "<1m"),
            (59, "<1m"),
            (60, "1m"),
            (3_599, "59m"),
            (3_600, "1h00m"),
            (9_630, "2h40m"),
            (86_399, "23h59m"),
            (86_400, "1d00h"),
            (291_630, "3d09h"),
        ] {
            assert_eq!(duration(secs), text, "{secs}");
        }
    }

    #[test]
    fn a_countdown_before_at_and_after_the_reset() {
        assert_eq!(countdown(NOW + 9_630, NOW), "2h40m", "before");
        assert_eq!(countdown(NOW + 60, NOW), "1m");
        assert_eq!(
            countdown(NOW + 1, NOW),
            "<1m",
            "a second ahead is still ahead"
        );
        assert_eq!(countdown(NOW, NOW), "reset", "at");
        assert_eq!(countdown(NOW - 1, NOW), "reset", "after");
        assert_eq!(countdown(NOW - 86_400, NOW), "reset");
    }

    #[test]
    fn severity_changes_at_70_and_90() {
        for (pct, expected) in [
            (-5, Severity::Normal),
            (0, Severity::Normal),
            (69, Severity::Normal),
            (70, Severity::Warning),
            (89, Severity::Warning),
            (90, Severity::Critical),
            (100, Severity::Critical),
            (250, Severity::Critical),
        ] {
            assert_eq!(severity(pct), expected, "{pct}");
        }
    }

    #[test]
    fn money_uses_the_symbol_or_the_code() {
        assert_eq!(money(0.0, "EUR"), "€0");
        assert_eq!(money(20.0, "eur"), "€20");
        assert_eq!(money(12.5, "USD"), "$12.50");
        assert_eq!(money(3.0, "GBP"), "£3");
        assert_eq!(money(7.25, "CHF"), "7.25 CHF");
        assert_eq!(
            money(7.0, "chf"),
            "7 CHF",
            "a code is shown in capitals, whole without cents"
        );
    }

    #[test]
    fn money_rounds_to_cents_before_dropping_a_whole_amounts_cents() {
        assert_eq!(money(12.999, "EUR"), "€13");
        assert_eq!(money(0.004, "EUR"), "€0");
        assert_eq!(money(19.996, "USD"), "$20");
        assert_eq!(money(0.006, "EUR"), "€0.01");
        assert_eq!(money(12.994, "EUR"), "€12.99");
        assert_eq!(money(-0.004, "EUR"), "€0");
    }

    #[test]
    fn spend_text_needs_a_spend_window_with_its_amounts() {
        assert_eq!(
            spend_text(&spend(0.0, 20.0, "EUR")).as_deref(),
            Some("€0 of €20")
        );
        assert_eq!(
            spend_text(&spend(3.5, 20.0, "USD")).as_deref(),
            Some("$3.50 of $20")
        );
        let mut no_currency = spend(1.0, 2.0, "EUR");
        no_currency.detail = Some(json!({"used": 1.0, "limit": 2.0}));
        assert_eq!(spend_text(&no_currency), None);
        let mut no_detail = spend(1.0, 2.0, "EUR");
        no_detail.detail = None;
        assert_eq!(spend_text(&no_detail), None);
        let mut not_spend = spend(1.0, 2.0, "EUR");
        not_spend.kind = WindowKind::Scoped;
        assert_eq!(
            spend_text(&not_spend),
            None,
            "only a spend window has amounts"
        );
    }

    #[test]
    fn every_status_says_when_it_is_retried_while_that_is_ahead() {
        let words_of = |status, error: Option<&str>, retry_in: Option<i64>| {
            usage_words(&unread(status, error, retry_in), NOW)
        };
        for (status, text) in [
            (UsageStatus::Ok, "no usage reported"),
            (UsageStatus::KeychainUnavailable, "keychain unavailable"),
            (UsageStatus::TokenExpired, "token expired"),
            (UsageStatus::NoCredentials, "no credentials"),
            (UsageStatus::ForeignCredential, "foreign credential"),
            (UsageStatus::ReloginRequired, "relogin required"),
            (UsageStatus::ApiKey, "api key"),
            (UsageStatus::Unsupported, "usage unsupported"),
        ] {
            assert_eq!(
                words_of(status, None, Some(120)),
                format!("{text} (retry 2m)")
            );
            assert_eq!(words_of(status, None, None), text);
            assert_eq!(
                words_of(status, None, Some(-5)),
                text,
                "a passed retry says nothing"
            );
            assert_eq!(
                words_of(status, None, Some(0)),
                text,
                "a retry that is due now says nothing"
            );
        }
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some("over-budget"), Some(2_400)),
            "over budget (retry 40m)"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some("over-budget"), None),
            "over budget"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some(NO_DATA), Some(30)),
            "no data yet (retry <1m)"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, None, None),
            "no data yet"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some("http-429"), Some(330)),
            "unavailable (http-429, retry 5m)"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some("http-429"), None),
            "unavailable (http-429)"
        );
    }

    #[test]
    fn columns_are_the_windows_some_reading_has_in_kind_order_and_spend_is_always_there() {
        assert_eq!(window_columns(&[]), vec![], "no accounts, no columns");
        let unread_only = view(
            1,
            "a@x.co",
            OAUTH,
            unread(UsageStatus::Unavailable, None, None),
        );
        assert_eq!(
            window_columns(&[unread_only.clone()]),
            vec![],
            "no reading, no columns"
        );
        // Scoped first in the reading and the 7d before the 5h: the kinds decide the order.
        let reading = read(vec![fable(), seven_day(), five_hour()]);
        let heads = |cols: Vec<WindowColumn>| -> Vec<(Option<String>, String)> {
            cols.into_iter().map(|c| (c.key, c.head)).collect()
        };
        assert_eq!(
            heads(window_columns(&[
                unread_only.clone(),
                view(2, "b@x.co", OAUTH, reading)
            ])),
            [
                (Some("5h".to_owned()), "5H".to_owned()),
                (Some("7d".to_owned()), "7D".to_owned()),
                (None, "SPEND".to_owned()),
                (Some("scoped:Fable".to_owned()), "FABLE".to_owned()),
            ],
            "SPEND is a column of its own even with no spend window"
        );
        // A spend window is a column of its own key, in the same place; the placeholder goes.
        let with_spend = read(vec![five_hour(), spend(0.0, 20.0, "EUR"), fable()]);
        let cols = window_columns(&[view(1, "a@x.co", OAUTH, with_spend)]);
        assert_eq!(
            cols.iter()
                .map(|c| (c.key.as_deref(), c.kind))
                .collect::<Vec<_>>(),
            [
                (Some("5h"), WindowKind::Short),
                (Some("spend"), WindowKind::Spend),
                (Some("scoped:Fable"), WindowKind::Scoped),
            ]
        );
        // A key two readings share is one column; scoped windows come in first-seen order.
        let a = read(vec![
            five_hour(),
            window("scoped:Zed", "Zed", WindowKind::Scoped, 1.0, None),
        ]);
        let b = read(vec![five_hour(), fable()]);
        let cols = window_columns(&[view(1, "a@x.co", OAUTH, a), view(2, "b@x.co", OAUTH, b)]);
        assert_eq!(
            cols.iter().map(|c| c.head.as_str()).collect::<Vec<_>>(),
            ["5H", "SPEND", "ZED", "FABLE"]
        );
    }

    #[test]
    fn a_rows_notes_are_its_status_kind_disabled_and_this_in_that_order() {
        let ok = view(1, "a@x.co", OAUTH, read(vec![five_hour()]));
        assert_eq!(
            row_notes(&ok, false, NOW),
            Vec::<String>::new(),
            "a good reading says nothing"
        );
        assert_eq!(row_notes(&ok, true, NOW), ["this"]);
        // A reading with a status that is not `ok` says it beside the windows.
        let mut locked = read(vec![five_hour()]);
        locked.status = UsageStatus::KeychainUnavailable;
        locked.retry_at = Some(NOW + 120);
        let beside = view(1, "a@x.co", OAUTH, locked);
        assert_eq!(
            row_notes(&beside, false, NOW),
            ["keychain unavailable (retry 2m)"]
        );
        // No reading: the words stand in for the windows, the kind follows, then the marks.
        let mut setup = view(
            4,
            "setup-token-4@token.local",
            SETUP_TOKEN,
            unread(UsageStatus::Unavailable, Some("pre-send"), Some(30)),
        );
        setup.row.disabled = true;
        assert_eq!(
            row_notes(&setup, true, NOW),
            [
                "unavailable (pre-send, retry <1m)",
                "setup token",
                "disabled",
                "this"
            ]
        );
        // A kind the words already say is not said twice.
        let mut key = view(
            5,
            "k@x.co",
            SETUP_TOKEN,
            unread(UsageStatus::Ok, None, None),
        );
        key.kind.display = Some("no usage reported");
        assert_eq!(row_notes(&key, false, NOW), ["no usage reported"]);
        // An empty reading is no reading.
        let empty = view(6, "e@x.co", OAUTH, read(vec![]));
        assert_eq!(row_notes(&empty, false, NOW), ["no usage reported"]);
    }

    #[test]
    fn a_quiet_windows_rate_keeps_two_decimals() {
        for (rate, text) in [
            (0.03, "+0.03"),
            (-0.049, "-0.05"),
            (0.005, "+0.01"),
            (0.004, "+<0.01"),
            (-1e-9, "-<0.01"),
            (0.0, "+0.0"),
            (-0.0, "+0.0"),
            (0.05, "+0.1"),
            (0.099, "+0.1"),
            (0.1, "+0.1"),
            (5.0, "+5.0"),
            (-1.26, "-1.3"),
        ] {
            assert_eq!(rate_signed(rate), text, "{rate}");
        }
    }

    #[test]
    fn a_projection_lasts_runs_out_or_is_missing() {
        let with = |pace: Pace| {
            let w = window("7d", "7d", WindowKind::Long, 30.0, None);
            projection_text(&history_window(w, pace), NOW)
        };
        assert_eq!(with(Pace::default()), "no rate yet");
        let past = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW - 60),
            ..Pace::default()
        };
        assert_eq!(
            with(past),
            "+5.0 pts/h · should have run out 1m ago (regression)"
        );
        let just_past = Pace {
            exhaustion_at: Some(NOW),
            ..past
        };
        assert_eq!(
            with(just_past),
            "+5.0 pts/h · should have run out <1m ago (regression)"
        );
        let soon = Pace {
            exhaustion_at: Some(NOW + 60),
            ..past
        };
        assert_eq!(with(soon), "+5.0 pts/h · runs out in 1m (regression)");
        let lasts = Pace {
            will_last_to_reset: Some(true),
            ..soon
        };
        assert_eq!(with(lasts), "+5.0 pts/h · lasts to reset (regression)");
        let flat = Pace {
            rate_per_hour: Some(0.0),
            method: Some(ProjectionMethod::Average),
            ..Pace::default()
        };
        assert_eq!(with(flat), "+0.0 pts/h · no projection (average)");
    }

    #[test]
    fn an_exhausted_window_is_at_the_limit_with_or_without_a_rate() {
        let exhausted = |pace: Pace| {
            let w = window("5h", "5h", WindowKind::Short, 100.0, Some(HOUR));
            projection_text(&history_window(w, pace), NOW)
        };
        // Read 100, 100, 100: no slope, no average for a Short window, but exhausted at the read.
        let no_rate = Pace {
            exhaustion_at: Some(NOW - 2 * HOUR),
            ..Pace::default()
        };
        assert_eq!(exhausted(no_rate), "at the limit");
        // With a rate the exhaustion time is the read's, hours ago: no countdown.
        let with_rate = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Average),
            exhaustion_at: Some(NOW - 2 * HOUR),
            will_last_to_reset: Some(false),
            ..Pace::default()
        };
        assert_eq!(exhausted(with_rate), "+5.0 pts/h · at the limit (average)");
    }

    #[test]
    fn a_projection_beyond_a_year_says_it_will_not_run_out() {
        let with = |pace: Pace, reset: Option<i64>| {
            let w = window("7d", "7d", WindowKind::Long, 30.0, reset);
            projection_text(&history_window(w, pace), NOW)
        };
        // A quiet window: 70 points to go at 0.006 an hour is well over a year.
        let tiny = Pace {
            rate_per_hour: Some(0.006),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW + 42_000_000),
            will_last_to_reset: None,
            ..Pace::default()
        };
        assert_eq!(
            with(tiny, None),
            "+0.01 pts/h · won't run out within a year (regression)"
        );
        // With a reset it will not reach, the reset is what counts (§8.7).
        let lasts = Pace {
            will_last_to_reset: Some(true),
            ..tiny
        };
        assert_eq!(
            with(lasts, Some(HOUR)),
            "+0.01 pts/h · lasts to reset (regression)"
        );
        // 70 points at 0.01 an hour is 291 days: a countdown still.
        let slow = Pace {
            rate_per_hour: Some(0.01),
            exhaustion_at: Some(NOW + 25_200_000),
            ..tiny
        };
        assert_eq!(
            with(slow, None),
            "+0.01 pts/h · runs out in 291d16h (regression)"
        );
        // The year itself is still a countdown; a second more is not.
        let year = |s| Pace {
            exhaustion_at: Some(NOW + 365 * 86_400 + s),
            ..tiny
        };
        assert_eq!(
            with(year(0), None),
            "+0.01 pts/h · runs out in 365d00h (regression)"
        );
        assert_eq!(
            with(year(1), None),
            "+0.01 pts/h · won't run out within a year (regression)"
        );
        // A saturated time must not overflow.
        let saturated = Pace {
            exhaustion_at: Some(i64::MAX),
            ..tiny
        };
        assert_eq!(
            with(saturated, None),
            "+0.01 pts/h · won't run out within a year (regression)"
        );
    }

    #[test]
    fn counts_scale_to_one_decimal_from_a_thousand() {
        for (n, text) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1.0K"),
            (1_234, "1.2K"),
            (1_250, "1.3K"),
            (51_000, "51.0K"),
            (999_949, "999.9K"),
            (999_950, "1.0M"),
            (12_400_000, "12.4M"),
            (3_100_000_000, "3.1B"),
            (999_950_000_000, "1.0T"),
            (u64::MAX, "18446744.1T"),
        ] {
            assert_eq!(scaled(n), text, "{n}");
        }
    }

    #[test]
    fn costs_are_dollars_and_cents_rounded_half_up() {
        assert_eq!(dollars(0), "$0.00");
        assert_eq!(dollars(4_999_999), "$0.00");
        assert_eq!(dollars(5_000_000), "$0.01");
        assert_eq!(dollars(622_016_000), "$0.62");
        assert_eq!(dollars(312_400_000_000), "$312.40");
        assert_eq!(dollars(u64::MAX), "$18446744073.71");
    }

    fn report() -> TokenReport {
        use crate::tokens::{IndexState, TokenGrouping, TokenPeriod, TokenQuery, TokenTotal};
        TokenReport {
            query: TokenQuery {
                provider: ProviderId::new(CLAUDE_CODE),
                period: TokenPeriod::Day,
                grouping: TokenGrouping::Account,
                since_ms: None,
                until_ms: None,
                accounts: None,
                models: None,
                session: None,
                tz: jiff::tz::TimeZone::UTC,
            },
            time_zone: "UTC".into(),
            rows: vec![],
            total: TokenTotal::default(),
            index: IndexState::default(),
            unpriced_models: vec![],
            prices_as_of: "2026-10-01",
            price_overrides: vec![],
        }
    }

    #[test]
    fn the_token_footnotes_name_what_the_report_leaves_out_and_mark_partial_costs_as_asked() {
        let plain = report();
        assert_eq!(
            token_footnotes(&plain, "*", true),
            ["API cost at list prices as of 2026-10-01."]
        );
        let mut every = report();
        every.unpriced_models = vec!["claude-test-9".into(), "claude-x".into()];
        every.total.unfinalized = 1;
        every.index.skipped_lines = 12;
        every.index.unreadable_files = 3;
        every.price_overrides = vec!["claude-opus-5-5".into()];
        assert_eq!(
            token_footnotes(&every, "≥", true),
            [
                "No price for claude-test-9, claude-x: counted without cost (≥ marks a partial cost).",
                "Output is a lower bound for 1 subagent response.",
                "12 transcript lines could not be read, so these totals may be low.",
                "3 transcript files could not be read; the next refresh tries again.",
                "API cost at list prices as of 2026-10-01, with config.toml's for claude-opus-5-5.",
            ]
        );
        assert_eq!(
            token_footnotes(&every, "*", false)[0],
            "No price for claude-test-9, claude-x: counted without cost (* marks a partial cost)."
        );
        assert_eq!(
            token_footnotes(&every, "*", false).len(),
            4,
            "the CLI warns of unreadable files on stderr instead (token report §11.3)"
        );
        assert_eq!(
            unreadable_transcripts(1),
            "1 transcript file could not be read; the next refresh tries again"
        );
        assert_eq!(plural(2, "subagent response"), "2 subagent responses");
    }
}
