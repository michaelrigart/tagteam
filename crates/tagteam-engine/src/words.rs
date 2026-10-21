//! The words `list`, `status`, `history` and `tokens` print, shared with the TUI so that both
//! say the same things (TUI spec §5.2, §5.3). They moved here from the CLI's `render.rs`,
//! `history.rs` and `tokens_cmd.rs` with their bodies unchanged; the CLI adds its ANSI colours
//! around them.

use tagteam_core::autoswitch::{NoSwitchReason, Trigger};
use tagteam_core::{Window, WindowKind};
use tagteam_provider::TransientKind;

use crate::failure::FailureKind;
use crate::store::AccountRow;
use crate::tokens::TokenReport;
use crate::views::{AccountView, HistoryWindow, NO_DATA, ProviderNames, UsageStatus, UsageView};

/// A window a reading does not have, or an age that is not known.
pub const MISSING: &str = "—";
/// §8.7's marker in `list` and `status` (§11.1): was `▲ pace`.
pub const AHEAD: &str = "▲ fast";
/// `list`'s head for a reading's age (§11.1): was `AGE`.
pub const UPDATED_HEAD: &str = "UPDATED";
/// An account never read (§11.1): was `no data yet`, and `never fetched` in the TUI.
pub const NOT_CHECKED: &str = "not checked yet";
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
pub use tagteam_core::fmt::span as duration;

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

// The status heads a problem's sentence restates (§11.1, T3-12).
const LOGIN_EXPIRED: &str = "login expired";
const KEYCHAIN: &str = "can't read the Keychain";
const NEEDS_LOGIN: &str = "needs a new login";
const OTHER_ACCOUNT: &str = "logged in as another account";
const NO_LOGIN: &str = "no stored login";
const RATE_LIMITED: &str = "rate-limited";
const BUDGET: &str = "paused to stay within the request budget";
const CHECK_FAILED: &str = "usage check failed";

/// A usage status's head and, for a failure said by its token, that token (T3-8).
fn status_parts(u: &UsageView) -> (&'static str, Option<String>) {
    match u.status {
        UsageStatus::Ok => ("no usage reported", None),
        UsageStatus::TokenExpired => (LOGIN_EXPIRED, None),
        UsageStatus::ApiKey => ("api key", None),
        UsageStatus::KeychainUnavailable => (KEYCHAIN, None),
        UsageStatus::ReloginRequired => (NEEDS_LOGIN, None),
        UsageStatus::ForeignCredential => (OTHER_ACCOUNT, None),
        UsageStatus::NoCredentials => (NO_LOGIN, None),
        UsageStatus::Unsupported => ("usage unsupported", None),
        UsageStatus::Unavailable => match u.error.as_deref() {
            None | Some(NO_DATA) => (NOT_CHECKED, None),
            Some(e) => match FailureKind::parse(e) {
                FailureKind::Provider(TransientKind::Http(429)) => (RATE_LIMITED, None),
                FailureKind::OverBudget => (BUDGET, None),
                _ => (CHECK_FAILED, Some(e.to_owned())),
            },
        },
    }
}

/// `head`, and `detail` in brackets when there is any.
fn with_detail(head: &str, detail: Vec<String>) -> String {
    if detail.is_empty() {
        head.to_owned()
    } else {
        format!("{head} ({})", detail.join(", "))
    }
}

/// A usage status in words without its retry (§11.1, T3-8): `needs a new login`,
/// `rate-limited`, `usage check failed (refresh-failed)`, `not checked yet`.
pub fn status_words(u: &UsageView) -> String {
    let (head, why) = status_parts(u);
    with_detail(head, why.into_iter().collect())
}

/// A usage status in words (§13.1): in place of a row's windows when it has no reading, and
/// beside them when its status is not `ok`. Whatever the status, a retry still ahead is said:
/// `can't read the Keychain (retry 2m)`, `rate-limited (retry 5m)`,
/// `usage check failed (refresh-failed, retry 5m)`.
pub fn usage_words(u: &UsageView, now_s: i64) -> String {
    let (head, why) = status_parts(u);
    let retry = u
        .retry_at
        .filter(|&at| at > now_s)
        .map(|at| format!("retry {}", duration(at - now_s)));
    with_detail(head, why.into_iter().chain(retry).collect())
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

/// A projection's clause (§11.1): `lasts until it resets` (`lasts` when `short`),
/// `full in 2d04h`, `should have been full 3h ago`, `won't fill within a year`,
/// `at the limit`, `not enough data yet`, `no projection`.
pub fn projection_clause(hw: &HistoryWindow, now_s: i64, short: bool) -> String {
    let p = &hw.pace;
    // An exhausted window ran out when it was read, which may be long ago: no countdown applies.
    if hw.window.pct >= 100.0 {
        return "at the limit".to_owned();
    }
    if p.rate_per_hour.is_none() {
        return "not enough data yet".to_owned();
    }
    match (p.will_last_to_reset, p.exhaustion_at) {
        (Some(true), _) if short => "lasts".to_owned(),
        (Some(true), _) => "lasts until it resets".to_owned(),
        (_, Some(at)) if at <= now_s => format!(
            "should have been full {} ago",
            duration(now_s.saturating_sub(at))
        ),
        (_, Some(at)) if at.saturating_sub(now_s) > A_YEAR_S => {
            "won't fill within a year".to_owned()
        }
        (_, Some(at)) => format!("full in {}", duration(at - now_s)),
        _ => "no projection".to_owned(),
    }
}

/// The burn rate and where it leads (§8.7): `+1.2%/h · lasts until it resets`, or the clause
/// alone without a rate. The method that measured the rate is the help's to name (T3-9).
pub fn projection_text(hw: &HistoryWindow, now_s: i64) -> String {
    let clause = projection_clause(hw, now_s, false);
    match hw.pace.rate_per_hour {
        Some(rate) => format!("{}%/h · {clause}", rate_signed(rate)),
        None => clause,
    }
}

/// `▲ faster than an even pace (62% expected by now)`, or without the parenthesis.
pub fn ahead_words(expected_pct: Option<f64>) -> String {
    match expected_pct {
        Some(e) => format!(
            "▲ faster than an even pace ({}% expected by now)",
            e.round() as i64
        ),
        None => "▲ faster than an even pace".to_owned(),
    }
}

/// `updated 2m ago`.
pub fn updated(age_s: i64) -> String {
    format!("updated {} ago", duration(age_s))
}

/// `next update in 1m`; `next update in <1m` once `at` has come, until the update lands.
pub fn next_update(at: i64, now_s: i64) -> String {
    format!("next update in {}", duration(at.saturating_sub(now_s)))
}

/// T3-10: `login valid 6 more days`, `… 1 more day`, `… 5h20m more`, `login expired`.
pub fn login_valid(expires_at_ms: i64, now_s: i64) -> String {
    const DAY: i64 = 86_400;
    let left = expires_at_ms.div_euclid(1000).saturating_sub(now_s);
    if left <= 0 {
        "login expired".to_owned()
    } else if left < DAY {
        format!("login valid {} more", duration(left))
    } else if left < 2 * DAY {
        "login valid 1 more day".to_owned()
    } else {
        format!("login valid {} more days", left / DAY)
    }
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

/// Transcript lines the token index skipped, so its totals may be low (token report §11.2).
pub fn skipped_transcript_lines(n: u64) -> String {
    format!(
        "{} could not be read, so these totals may be low.",
        plural(n, "transcript line")
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
        notes.push(skipped_transcript_lines(report.index.skipped_lines));
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

/// What a `no-switch` sentence says besides its reason (spec §11.3).
#[derive(Debug, Clone, Copy)]
pub struct NoSwitchFacts<'a> {
    /// The decision's `detail` as `auto` reports it: `4m` (cooldown), `2/3` (active-usage-unknown); else empty.
    pub detail: &'a str,
    /// The account in use's usage at the tick, for `below-threshold`: 100 less its headroom.
    pub used_pct: Option<f64>,
    pub threshold: f64,
    /// `all-exhausted`: when the first account frees up, in the caller's clock form (`16:40`).
    pub frees_at: Option<&'a str>,
    pub display_name: &'a str,
}

/// A threshold as the header and the sentences show it: `90%`, or `92.5%` with a fraction.
pub fn threshold(pct: f64) -> String {
    if pct.fract() == 0.0 {
        format!("{pct:.0}%")
    } else {
        format!("{pct:.1}%")
    }
}

/// Auto's decision in plain words (spec §11.3), for the auto line and `tagteam auto`.
pub fn no_switch(reason: NoSwitchReason, facts: &NoSwitchFacts<'_>) -> String {
    let t = threshold(facts.threshold);
    let display = facts.display_name;
    match reason {
        NoSwitchReason::BelowThreshold => match facts.used_pct {
            Some(u) => format!("no switch needed ({}%, switches at {t})", u.round() as i64),
            None => format!("no switch needed (switches at {t})"),
        },
        NoSwitchReason::Cooldown if facts.detail.is_empty() => {
            "waiting after the last switch".into()
        }
        NoSwitchReason::Cooldown => format!("waiting {} after the last switch", facts.detail),
        NoSwitchReason::AllExhausted => match facts.frees_at {
            Some(at) => format!("every account is near its limit; the first frees up at {at}"),
            None => "every account is near its limit".into(),
        },
        NoSwitchReason::NoQualifyingCandidate => "no other account has enough headroom".into(),
        NoSwitchReason::NoCandidates => "no other account can be used".into(),
        NoSwitchReason::NoComparison => "no other account has a reading to compare".into(),
        NoSwitchReason::StaleUsage => {
            "the best account's reading is too old; checking it first".into()
        }
        NoSwitchReason::ActiveUsageUnknown => match facts.detail.split_once('/') {
            Some((n, m)) => format!("can't read the account in use ({n} of {m} tries)"),
            None => "can't read the account in use".into(),
        },
        NoSwitchReason::UnmanagedActiveAccount => {
            format!("{display} is logged in as an account tagteam doesn't manage")
        }
        NoSwitchReason::NoActiveAccount => format!("{display} isn't logged in"),
        NoSwitchReason::ActiveApiKey => {
            "the account in use is an API key, which auto leaves alone".into()
        }
        NoSwitchReason::ResetUnknown => "can't tell when the account in use resets".into(),
        NoSwitchReason::AlreadyConsumingSoonest => {
            "already on the account that resets first".into()
        }
        NoSwitchReason::EngineRunning => "auto already runs in another process".into(),
        NoSwitchReason::InterruptedSwitch => "finishing an interrupted switch first".into(),
        NoSwitchReason::LiveChanged => {
            "the login changed meanwhile; trying again next check".into()
        }
        NoSwitchReason::NoViableTarget => "every target failed; trying again next check".into(),
    }
}

/// §11.3's trigger words, for switches that predate `detail`.
pub fn trigger(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Proactive => "before it reached the limit",
        Trigger::AtLimit => "at the limit",
        Trigger::Failover => "its usage couldn't be read",
        Trigger::ConsumeFirst => "to use up the account that resets first",
    }
}

/// `<position> <alias, else the email's local part>`: `2 work`, `1 alex` (spec §5.3, §11.4).
pub fn short_name(position: u32, alias: Option<&str>, email_or_label: &str) -> String {
    let name = alias.unwrap_or_else(|| email_or_label.split('@').next().unwrap_or(email_or_label));
    format!("{position} {name}")
}

/// What needs attention on an account (redesign §6.3, §11.2, T3-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    NeedsLogin,
    LoginExpired,
    NoLogin,
    RateLimited,
    Keychain,
    Budget,
    OtherAccount,
    Retrying,
    OldReading,
    NotChecked,
}

impl Problem {
    /// Every problem once, in declaration order.
    pub const ALL: [Problem; 10] = [
        Problem::NeedsLogin,
        Problem::LoginExpired,
        Problem::NoLogin,
        Problem::RateLimited,
        Problem::Keychain,
        Problem::Budget,
        Problem::OtherAccount,
        Problem::Retrying,
        Problem::OldReading,
        Problem::NotChecked,
    ];
}

/// The account's one problem, from its usage: `None` for a healthy reading, an API key or an
/// unsupported provider. Disabled is not a problem.
pub fn problem(v: &AccountView) -> Option<Problem> {
    let u = &v.usage;
    match u.status {
        UsageStatus::ReloginRequired => Some(Problem::NeedsLogin),
        UsageStatus::TokenExpired => Some(Problem::LoginExpired),
        UsageStatus::NoCredentials => Some(Problem::NoLogin),
        UsageStatus::KeychainUnavailable => Some(Problem::Keychain),
        UsageStatus::ForeignCredential => Some(Problem::OtherAccount),
        UsageStatus::Unavailable => Some(match u.error.as_deref() {
            None | Some(NO_DATA) => Problem::NotChecked,
            Some(e) => match FailureKind::parse(e) {
                FailureKind::Provider(TransientKind::Http(429)) => Problem::RateLimited,
                FailureKind::OverBudget => Problem::Budget,
                _ => Problem::Retrying,
            },
        }),
        UsageStatus::Ok if u.windows.is_some() && !u.decision_grade => Some(Problem::OldReading),
        UsageStatus::Ok | UsageStatus::ApiKey | UsageStatus::Unsupported => None,
    }
}

/// `words` with its first letter in capitals, to open a sentence.
fn capitalised(words: &str) -> String {
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// T3-12's sentence. `when` is the quarantine's time in the caller's form (`Sat 09:02`).
pub fn problem_sentence(problem: Problem, v: &AccountView, when: Option<&str>) -> String {
    let (head, cause) = match problem {
        Problem::NeedsLogin => {
            let reason = v
                .row
                .quarantine_reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            let when = when.map(|w| format!(", {w}")).unwrap_or_default();
            let cause = format!("its stored login was refused{reason}{when}");
            return format!("{}: {cause}.", capitalised(NEEDS_LOGIN));
        }
        Problem::LoginExpired => (
            LOGIN_EXPIRED,
            "its stored login ran out and could not be renewed",
        ),
        Problem::NoLogin => (NO_LOGIN, "tagteam holds no login for this account"),
        Problem::Keychain => (KEYCHAIN, "it is locked, or it refused tagteam"),
        Problem::RateLimited => (RATE_LIMITED, "the usage check was turned away for now"),
        Problem::OtherAccount => (
            OTHER_ACCOUNT,
            "the login tagteam read belongs to a different account",
        ),
        Problem::OldReading => ("old reading", "too old to pick the best account by"),
        Problem::Budget => return format!("{}.", capitalised(BUDGET)),
        Problem::Retrying => {
            let token = v.usage.error.clone().into_iter().collect();
            return format!("{}.", capitalised(&with_detail(CHECK_FAILED, token)));
        }
        Problem::NotChecked => return format!("{}.", capitalised(NOT_CHECKED)),
    };
    format!("{}: {cause}.", capitalised(head))
}

/// §11.2's remedy (T3-13): `Log in to <display name> as alex@example.com, then press A here.`
pub fn remedy(
    problem: Problem,
    v: &AccountView,
    names: &ProviderNames,
    now_s: i64,
) -> Option<String> {
    let ahead = |at: Option<i64>| at.filter(|&at| at > now_s).map(|at| duration(at - now_s));
    let u = &v.usage;
    let display = names.display_name;
    Some(match problem {
        Problem::NeedsLogin | Problem::LoginExpired | Problem::NoLogin => format!(
            "Log in to {display} as {}, then press A here.",
            email(&v.row)
        ),
        Problem::Keychain => "Unlock it (security unlock-keychain), then press r.".into(),
        Problem::RateLimited | Problem::Retrying => {
            format!("tagteam tries again in {}.", ahead(u.retry_at)?)
        }
        Problem::Budget => match ahead(u.retry_at) {
            Some(span) => format!("tagteam checks again in {span}, when the hourly budget allows."),
            None => "tagteam checks again when the hourly budget allows.".into(),
        },
        Problem::OtherAccount => format!(
            "{display} is logged in as a different account. Switch with s, or log in again."
        ),
        Problem::NotChecked | Problem::OldReading => {
            match ahead(u.next_poll_at).or_else(|| ahead(u.retry_at)) {
                Some(span) => format!("tagteam checks it within {span}."),
                None => "Press r to check it now.".into(),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tagteam_cc::CLAUDE_CODE;
    use tagteam_core::{AccountId, Pace, ProjectionMethod, ProviderId};
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
    fn every_usage_status_has_its_words() {
        use UsageStatus::*;
        let expected = [
            (Ok, "no usage reported"),
            (TokenExpired, "login expired"),
            (ApiKey, "api key"),
            (KeychainUnavailable, "can't read the Keychain"),
            (ReloginRequired, "needs a new login"),
            (ForeignCredential, "logged in as another account"),
            (NoCredentials, "no stored login"),
            (Unavailable, "not checked yet"),
            (Unsupported, "usage unsupported"),
        ];
        assert_eq!(expected.len(), UsageStatus::ALL.len());
        for status in UsageStatus::ALL {
            let want = expected.iter().find(|(s, _)| *s == status).unwrap().1;
            assert_eq!(
                status_words(&unread(status, None, None)),
                want,
                "{status:?}"
            );
        }
        assert_eq!(NOT_CHECKED, "not checked yet");
    }

    #[test]
    fn the_status_list_is_every_status_once() {
        use UsageStatus::*;
        fn index(s: UsageStatus) -> usize {
            match s {
                Ok => 0,
                TokenExpired => 1,
                ApiKey => 2,
                KeychainUnavailable => 3,
                ReloginRequired => 4,
                ForeignCredential => 5,
                NoCredentials => 6,
                Unavailable => 7,
                Unsupported => 8,
            }
        }
        assert_eq!(UsageStatus::ALL.map(index), [0, 1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn a_failure_token_is_said_in_its_kind_s_words() {
        let words_of = |error: &str, retry_in: Option<i64>| {
            usage_words(
                &unread(UsageStatus::Unavailable, Some(error), retry_in),
                NOW,
            )
        };
        assert_eq!(words_of("http-429", Some(300)), "rate-limited (retry 5m)");
        assert_eq!(
            words_of("refresh-failed", Some(300)),
            "usage check failed (refresh-failed, retry 5m)"
        );
        assert_eq!(words_of("pre-send", None), "usage check failed (pre-send)");
        assert_eq!(
            words_of("over-budget", Some(720)),
            "paused to stay within the request budget (retry 12m)"
        );
        assert_eq!(words_of("http-503", None), "usage check failed (http-503)");
        assert_eq!(words_of(NO_DATA, None), "not checked yet");
        // Every other kind is said by its token.
        for token in [
            "http-401",
            "profile-drifted",
            "live-replaced",
            "ambiguous",
            "bad-response",
        ] {
            assert!(
                !matches!(
                    FailureKind::parse(token),
                    FailureKind::Other(_) | FailureKind::OverBudget
                ),
                "{token} is a kind of its own"
            );
            assert_eq!(
                words_of(token, None),
                format!("usage check failed ({token})")
            );
        }
        // Without its retry, as the TUI's card says it.
        let failed = unread(UsageStatus::Unavailable, Some("refresh-failed"), Some(300));
        assert_eq!(status_words(&failed), "usage check failed (refresh-failed)");
        let limited = unread(UsageStatus::Unavailable, Some("http-429"), Some(300));
        assert_eq!(status_words(&limited), "rate-limited");
    }

    #[test]
    fn every_status_says_when_it_is_retried_while_that_is_ahead() {
        let words_of = |status, error: Option<&str>, retry_in: Option<i64>| {
            usage_words(&unread(status, error, retry_in), NOW)
        };
        for (status, text) in [
            (UsageStatus::Ok, "no usage reported"),
            (UsageStatus::KeychainUnavailable, "can't read the Keychain"),
            (UsageStatus::TokenExpired, "login expired"),
            (UsageStatus::NoCredentials, "no stored login"),
            (
                UsageStatus::ForeignCredential,
                "logged in as another account",
            ),
            (UsageStatus::ReloginRequired, "needs a new login"),
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
            "paused to stay within the request budget (retry 40m)"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some("over-budget"), None),
            "paused to stay within the request budget"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some(NO_DATA), Some(30)),
            "not checked yet (retry <1m)"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, None, None),
            "not checked yet"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some("http-429"), Some(330)),
            "rate-limited (retry 5m)"
        );
        assert_eq!(
            words_of(UsageStatus::Unavailable, Some("http-429"), None),
            "rate-limited"
        );
    }

    #[test]
    fn an_unknown_failure_token_is_unavailable_in_its_own_spelling() {
        assert_eq!(
            usage_words(
                &unread(
                    UsageStatus::Unavailable,
                    Some("a-token-from-a-newer-build"),
                    None
                ),
                NOW
            ),
            "usage check failed (a-token-from-a-newer-build)"
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
            ["can't read the Keychain (retry 2m)"]
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
                "usage check failed (pre-send, retry <1m)",
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
    fn a_projection_says_how_fast_and_when_the_window_is_full() {
        let with = |pace: Pace, pct: f64, reset: Option<i64>| {
            let w = window("7d", "7d", WindowKind::Long, pct, reset);
            projection_text(&history_window(w, pace), NOW)
        };
        let texts = [
            (Pace::default(), 30.0, None, "not enough data yet"),
            (
                Pace {
                    rate_per_hour: Some(1.2),
                    method: Some(ProjectionMethod::Regression),
                    exhaustion_at: Some(NOW + 50 * HOUR),
                    will_last_to_reset: Some(true),
                    ..Pace::default()
                },
                30.0,
                Some(HOUR),
                "+1.2%/h · lasts until it resets",
            ),
            (
                Pace {
                    rate_per_hour: Some(5.0),
                    method: Some(ProjectionMethod::Regression),
                    exhaustion_at: Some(NOW + 12 * HOUR),
                    will_last_to_reset: Some(false),
                    ..Pace::default()
                },
                30.0,
                None,
                "+5.0%/h · full in 12h00m",
            ),
            (
                Pace {
                    rate_per_hour: Some(5.0),
                    method: Some(ProjectionMethod::Regression),
                    exhaustion_at: Some(NOW - 3 * HOUR),
                    ..Pace::default()
                },
                30.0,
                None,
                "+5.0%/h · should have been full 3h00m ago",
            ),
            (
                Pace {
                    rate_per_hour: Some(0.03),
                    method: Some(ProjectionMethod::Average),
                    exhaustion_at: Some(NOW + 2 * A_YEAR_S),
                    ..Pace::default()
                },
                30.0,
                None,
                "+0.03%/h · won't fill within a year",
            ),
            // An exhausted window ran out when it was read, which may be long ago: no
            // countdown, with or without a rate.
            (
                Pace {
                    rate_per_hour: Some(1.0),
                    method: Some(ProjectionMethod::Average),
                    exhaustion_at: Some(NOW - 2 * HOUR),
                    will_last_to_reset: Some(false),
                    ..Pace::default()
                },
                100.0,
                Some(HOUR),
                "+1.0%/h · at the limit",
            ),
            (
                Pace {
                    exhaustion_at: Some(NOW - 2 * HOUR),
                    ..Pace::default()
                },
                100.0,
                Some(HOUR),
                "at the limit",
            ),
            (
                Pace {
                    rate_per_hour: Some(0.0),
                    method: Some(ProjectionMethod::Average),
                    ..Pace::default()
                },
                30.0,
                None,
                "+0.0%/h · no projection",
            ),
        ];
        for (pace, pct, reset, text) in texts {
            let said = with(pace, pct, reset);
            assert_eq!(said, text);
            for gone in ["regression", "average", "pts/h"] {
                assert!(!said.contains(gone), "{said}");
            }
        }
    }

    #[test]
    fn a_projection_s_edges_keep_their_countdowns() {
        let with = |pace: Pace, reset: Option<i64>| {
            let w = window("7d", "7d", WindowKind::Long, 30.0, reset);
            projection_text(&history_window(w, pace), NOW)
        };
        let past = Pace {
            rate_per_hour: Some(5.0),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW),
            ..Pace::default()
        };
        assert_eq!(with(past, None), "+5.0%/h · should have been full <1m ago");
        let soon = Pace {
            exhaustion_at: Some(NOW + 60),
            ..past
        };
        assert_eq!(with(soon, None), "+5.0%/h · full in 1m");
        // A quiet window: 70 points to go at 0.006 an hour is well over a year.
        let tiny = Pace {
            rate_per_hour: Some(0.006),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW + 42_000_000),
            will_last_to_reset: None,
            ..Pace::default()
        };
        assert_eq!(with(tiny, None), "+0.01%/h · won't fill within a year");
        // With a reset it will not reach, the reset is what counts (§8.7).
        let lasts = Pace {
            will_last_to_reset: Some(true),
            ..tiny
        };
        assert_eq!(with(lasts, Some(HOUR)), "+0.01%/h · lasts until it resets");
        // 70 points at 0.01 an hour is 291 days: a countdown still.
        let slow = Pace {
            rate_per_hour: Some(0.01),
            exhaustion_at: Some(NOW + 25_200_000),
            ..tiny
        };
        assert_eq!(with(slow, None), "+0.01%/h · full in 291d16h");
        // The year itself is still a countdown; a second more is not.
        let year = |s| Pace {
            exhaustion_at: Some(NOW + 365 * 86_400 + s),
            ..tiny
        };
        assert_eq!(with(year(0), None), "+0.01%/h · full in 365d00h");
        assert_eq!(with(year(1), None), "+0.01%/h · won't fill within a year");
        // A saturated time must not overflow.
        let saturated = Pace {
            exhaustion_at: Some(i64::MAX),
            ..tiny
        };
        assert_eq!(with(saturated, None), "+0.01%/h · won't fill within a year");
    }

    #[test]
    fn the_short_clause_is_lasts_or_the_full_time() {
        let clause = |pace: Pace, short: bool| {
            let w = window("7d", "7d", WindowKind::Long, 30.0, Some(HOUR));
            projection_clause(&history_window(w, pace), NOW, short)
        };
        let lasts = Pace {
            rate_per_hour: Some(1.2),
            method: Some(ProjectionMethod::Regression),
            exhaustion_at: Some(NOW + 50 * HOUR),
            will_last_to_reset: Some(true),
            ..Pace::default()
        };
        assert_eq!(clause(lasts, false), "lasts until it resets");
        assert_eq!(clause(lasts, true), "lasts");
        let full = Pace {
            rate_per_hour: Some(5.0),
            exhaustion_at: Some(NOW + 12 * HOUR),
            will_last_to_reset: Some(false),
            ..lasts
        };
        assert_eq!(clause(full, false), "full in 12h00m");
        assert_eq!(clause(full, true), "full in 12h00m");
    }

    #[test]
    fn ahead_words_say_the_expected_share() {
        assert_eq!(AHEAD, "▲ fast");
        assert_eq!(
            ahead_words(Some(62.0)),
            "▲ faster than an even pace (62% expected by now)"
        );
        assert_eq!(
            ahead_words(Some(61.6)),
            "▲ faster than an even pace (62% expected by now)"
        );
        assert_eq!(ahead_words(None), "▲ faster than an even pace");
    }

    #[test]
    fn updated_next_update_and_login_valid_read_as_the_tui_shows_them() {
        assert_eq!(UPDATED_HEAD, "UPDATED");
        assert_eq!(updated(120), "updated 2m ago");
        assert_eq!(next_update(NOW + 60, NOW), "next update in 1m");
        assert_eq!(next_update(NOW, NOW), "next update in <1m");
        assert_eq!(next_update(NOW - 5, NOW), "next update in <1m");
        let ms = |s: i64| s * 1_000;
        let day = 86_400;
        assert_eq!(
            login_valid(ms(NOW + 6 * day + 3 * HOUR), NOW),
            "login valid 6 more days"
        );
        assert_eq!(
            login_valid(ms(NOW + 2 * day), NOW),
            "login valid 2 more days"
        );
        assert_eq!(
            login_valid(ms(NOW + day + 2 * HOUR), NOW),
            "login valid 1 more day"
        );
        assert_eq!(login_valid(ms(NOW + day), NOW), "login valid 1 more day");
        assert_eq!(login_valid(ms(NOW + 30), NOW), "login valid <1m more");
        assert_eq!(login_valid(ms(NOW), NOW), "login expired");
        assert_eq!(
            login_valid(ms(NOW + 5 * HOUR + 20 * 60), NOW),
            "login valid 5h20m more"
        );
        assert_eq!(login_valid(ms(NOW - 1), NOW), "login expired");
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

    fn facts(detail: &str) -> NoSwitchFacts<'_> {
        NoSwitchFacts {
            detail,
            used_pct: Some(77.0),
            threshold: 90.0,
            frees_at: Some("16:40"),
            display_name: "Claude Code",
        }
    }

    #[test]
    fn every_no_switch_reason_has_its_sentence() {
        use NoSwitchReason::*;
        let expected = [
            (BelowThreshold, "no switch needed (77%, switches at 90%)"),
            (Cooldown, "waiting 4m after the last switch"),
            (
                AllExhausted,
                "every account is near its limit; the first frees up at 16:40",
            ),
            (
                NoQualifyingCandidate,
                "no other account has enough headroom",
            ),
            (NoCandidates, "no other account can be used"),
            (NoComparison, "no other account has a reading to compare"),
            (
                StaleUsage,
                "the best account's reading is too old; checking it first",
            ),
            (
                ActiveUsageUnknown,
                "can't read the account in use (2 of 3 tries)",
            ),
            (
                UnmanagedActiveAccount,
                "Claude Code is logged in as an account tagteam doesn't manage",
            ),
            (NoActiveAccount, "Claude Code isn't logged in"),
            (
                ActiveApiKey,
                "the account in use is an API key, which auto leaves alone",
            ),
            (ResetUnknown, "can't tell when the account in use resets"),
            (
                AlreadyConsumingSoonest,
                "already on the account that resets first",
            ),
            (EngineRunning, "auto already runs in another process"),
            (InterruptedSwitch, "finishing an interrupted switch first"),
            (
                LiveChanged,
                "the login changed meanwhile; trying again next check",
            ),
            (
                NoViableTarget,
                "every target failed; trying again next check",
            ),
        ];
        assert_eq!(expected.len(), NoSwitchReason::ALL.len());
        for reason in NoSwitchReason::ALL {
            let detail = match reason {
                Cooldown => "4m",
                ActiveUsageUnknown => "2/3",
                _ => "",
            };
            let want = expected.iter().find(|(r, _)| *r == reason).unwrap().1;
            assert_eq!(no_switch(reason, &facts(detail)), want, "{reason:?}");
        }
    }

    #[test]
    fn a_sentence_without_its_facts_still_reads() {
        let bare = NoSwitchFacts {
            detail: "",
            used_pct: None,
            threshold: 90.0,
            frees_at: None,
            display_name: "Claude Code",
        };
        assert_eq!(
            no_switch(NoSwitchReason::BelowThreshold, &bare),
            "no switch needed (switches at 90%)"
        );
        assert_eq!(
            no_switch(NoSwitchReason::Cooldown, &bare),
            "waiting after the last switch"
        );
        assert_eq!(
            no_switch(NoSwitchReason::AllExhausted, &bare),
            "every account is near its limit"
        );
        assert_eq!(
            no_switch(NoSwitchReason::ActiveUsageUnknown, &bare),
            "can't read the account in use"
        );
    }

    #[test]
    fn the_used_percentage_rounds_a_tie_up_as_list_does() {
        let tie = NoSwitchFacts {
            used_pct: Some(76.5),
            ..facts("")
        };
        assert_eq!(
            no_switch(NoSwitchReason::BelowThreshold, &tie),
            "no switch needed (77%, switches at 90%)"
        );
    }

    #[test]
    fn the_reason_list_is_every_reason_once() {
        use NoSwitchReason::*;
        fn index(r: NoSwitchReason) -> usize {
            match r {
                UnmanagedActiveAccount => 0,
                NoActiveAccount => 1,
                ActiveApiKey => 2,
                BelowThreshold => 3,
                ActiveUsageUnknown => 4,
                Cooldown => 5,
                NoCandidates => 6,
                NoComparison => 7,
                ResetUnknown => 8,
                AlreadyConsumingSoonest => 9,
                NoQualifyingCandidate => 10,
                StaleUsage => 11,
                AllExhausted => 12,
                EngineRunning => 13,
                InterruptedSwitch => 14,
                LiveChanged => 15,
                NoViableTarget => 16,
            }
        }
        assert_eq!(
            NoSwitchReason::ALL.map(index),
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
    }

    #[test]
    fn every_trigger_has_its_words() {
        assert_eq!(trigger(Trigger::Proactive), "before it reached the limit");
        assert_eq!(trigger(Trigger::AtLimit), "at the limit");
        assert_eq!(trigger(Trigger::Failover), "its usage couldn't be read");
        assert_eq!(
            trigger(Trigger::ConsumeFirst),
            "to use up the account that resets first"
        );
    }

    #[test]
    fn a_threshold_is_whole_unless_it_has_a_fraction() {
        assert_eq!(threshold(90.0), "90%");
        assert_eq!(threshold(92.5), "92.5%");
        assert_eq!(threshold(50.0), "50%");
    }

    #[test]
    fn a_short_name_is_the_alias_else_the_email_s_local_part() {
        assert_eq!(short_name(2, Some("work"), "sam@corp.example"), "2 work");
        assert_eq!(short_name(1, None, "alex@example.com"), "1 alex");
        assert_eq!(short_name(3, None, "ci-label"), "3 ci-label");
    }

    const NAMES: ProviderNames = ProviderNames {
        display_name: "Claude Code",
        launch_command: "claude",
    };

    /// An account with `problem`: quarantined for `invalid_grant` when it needs a new login,
    /// its retry 4m ahead (12m for the budget) and its next poll 12m ahead.
    fn with_problem(problem: Problem) -> AccountView {
        let failed = |error: &str, retry_in: i64| UsageView {
            next_poll_at: Some(NOW + 720),
            ..unread(UsageStatus::Unavailable, Some(error), Some(retry_in))
        };
        let usage = match problem {
            Problem::NeedsLogin => unread(UsageStatus::ReloginRequired, None, None),
            Problem::LoginExpired => unread(UsageStatus::TokenExpired, None, Some(240)),
            Problem::NoLogin => unread(UsageStatus::NoCredentials, None, Some(240)),
            Problem::RateLimited => failed("http-429", 240),
            Problem::Keychain => unread(UsageStatus::KeychainUnavailable, None, Some(240)),
            Problem::Budget => failed("over-budget", 720),
            Problem::OtherAccount => unread(UsageStatus::ForeignCredential, None, Some(240)),
            Problem::Retrying => failed("refresh-failed", 240),
            Problem::OldReading => UsageView {
                decision_grade: false,
                next_poll_at: Some(NOW + 720),
                ..read(vec![five_hour(), seven_day()])
            },
            Problem::NotChecked => failed(NO_DATA, 240),
        };
        let mut v = view(1, "alex@example.com", OAUTH, usage);
        if problem == Problem::NeedsLogin {
            v.row.quarantine_reason = Some("invalid_grant".into());
            v.row.quarantine_at = Some(NOW - HOUR);
        }
        v
    }

    #[test]
    fn every_problem_has_its_sentence_and_remedy() {
        use Problem::*;
        let login = "Log in to Claude Code as alex@example.com, then press A here.";
        let expected = [
            (
                NeedsLogin,
                "Needs a new login: its stored login was refused (invalid_grant), Sat 09:02.",
                Some(login),
            ),
            (
                LoginExpired,
                "Login expired: its stored login ran out and could not be renewed.",
                Some(login),
            ),
            (
                NoLogin,
                "No stored login: tagteam holds no login for this account.",
                Some(login),
            ),
            (
                RateLimited,
                "Rate-limited: the usage check was turned away for now.",
                Some("tagteam tries again in 4m."),
            ),
            (
                Keychain,
                "Can't read the Keychain: it is locked, or it refused tagteam.",
                Some("Unlock it (security unlock-keychain), then press r."),
            ),
            (
                Budget,
                "Paused to stay within the request budget.",
                Some("tagteam checks again in 12m, when the hourly budget allows."),
            ),
            (
                OtherAccount,
                "Logged in as another account: the login tagteam read belongs to a different account.",
                Some(
                    "Claude Code is logged in as a different account. Switch with s, or log in again.",
                ),
            ),
            (
                Retrying,
                "Usage check failed (refresh-failed).",
                Some("tagteam tries again in 4m."),
            ),
            (
                OldReading,
                "Old reading: too old to pick the best account by.",
                Some("tagteam checks it within 12m."),
            ),
            (
                NotChecked,
                "Not checked yet.",
                Some("tagteam checks it within 12m."),
            ),
        ];
        assert_eq!(expected.len(), Problem::ALL.len());
        for p in Problem::ALL {
            let (_, sentence, fix) = expected.iter().find(|(q, ..)| *q == p).unwrap();
            let v = with_problem(p);
            assert_eq!(problem(&v), Some(p), "{p:?}");
            assert_eq!(
                problem_sentence(p, &v, Some("Sat 09:02")),
                *sentence,
                "{p:?}"
            );
            assert_eq!(remedy(p, &v, &NAMES, NOW).as_deref(), *fix, "{p:?}");
        }
    }

    #[test]
    fn the_problem_list_is_every_problem_once() {
        use Problem::*;
        fn index(p: Problem) -> usize {
            match p {
                NeedsLogin => 0,
                LoginExpired => 1,
                NoLogin => 2,
                RateLimited => 3,
                Keychain => 4,
                Budget => 5,
                OtherAccount => 6,
                Retrying => 7,
                OldReading => 8,
                NotChecked => 9,
            }
        }
        assert_eq!(Problem::ALL.map(index), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn a_problem_follows_the_account_s_usage() {
        let unavailable = |error: Option<&str>| unread(UsageStatus::Unavailable, error, None);
        let old = UsageView {
            decision_grade: false,
            ..read(vec![five_hour()])
        };
        let rows = [
            (
                unread(UsageStatus::ReloginRequired, None, None),
                Some(Problem::NeedsLogin),
            ),
            (
                unread(UsageStatus::TokenExpired, None, None),
                Some(Problem::LoginExpired),
            ),
            (
                unread(UsageStatus::NoCredentials, None, None),
                Some(Problem::NoLogin),
            ),
            (
                unread(UsageStatus::KeychainUnavailable, None, None),
                Some(Problem::Keychain),
            ),
            (
                unread(UsageStatus::ForeignCredential, None, None),
                Some(Problem::OtherAccount),
            ),
            (unavailable(Some("http-429")), Some(Problem::RateLimited)),
            (unavailable(Some("over-budget")), Some(Problem::Budget)),
            (unavailable(Some(NO_DATA)), Some(Problem::NotChecked)),
            (unavailable(None), Some(Problem::NotChecked)),
            (unavailable(Some("refresh-failed")), Some(Problem::Retrying)),
            (unavailable(Some("http-503")), Some(Problem::Retrying)),
            (old, Some(Problem::OldReading)),
            (read(vec![five_hour(), seven_day()]), None),
            (unread(UsageStatus::ApiKey, None, None), None),
            (unread(UsageStatus::Unsupported, None, None), None),
        ];
        for (usage, want) in rows {
            let v = view(1, "alex@example.com", OAUTH, usage);
            assert_eq!(problem(&v), want, "{:?}", v.usage);
        }
        let mut disabled = view(1, "alex@example.com", OAUTH, read(vec![five_hour()]));
        disabled.row.disabled = true;
        assert_eq!(problem(&disabled), None, "disabled is not a problem");
    }

    #[test]
    fn an_unknown_failure_token_is_a_failed_check_with_no_remedy_until_a_retry() {
        let token = "a-token-from-a-newer-build";
        let mut v = view(
            1,
            "alex@example.com",
            OAUTH,
            unread(UsageStatus::Unavailable, Some(token), None),
        );
        assert_eq!(problem(&v), Some(Problem::Retrying));
        assert_eq!(
            problem_sentence(Problem::Retrying, &v, None),
            "Usage check failed (a-token-from-a-newer-build)."
        );
        assert_eq!(remedy(Problem::Retrying, &v, &NAMES, NOW), None);
        v.usage.retry_at = Some(NOW + 240);
        assert_eq!(
            remedy(Problem::Retrying, &v, &NAMES, NOW).as_deref(),
            Some("tagteam tries again in 4m.")
        );
    }

    #[test]
    fn a_problem_without_its_facts_still_reads() {
        let mut v = with_problem(Problem::NeedsLogin);
        assert_eq!(
            problem_sentence(Problem::NeedsLogin, &v, None),
            "Needs a new login: its stored login was refused (invalid_grant)."
        );
        v.row.quarantine_reason = None;
        v.row.quarantine_at = None;
        assert_eq!(
            problem_sentence(Problem::NeedsLogin, &v, Some("Sat 09:02")),
            "Needs a new login: its stored login was refused, Sat 09:02."
        );
        assert_eq!(
            problem_sentence(Problem::NeedsLogin, &v, None),
            "Needs a new login: its stored login was refused."
        );

        let unplanned = |p: Problem| {
            let mut v = with_problem(p);
            v.usage.retry_at = None;
            v.usage.next_poll_at = None;
            remedy(p, &v, &NAMES, NOW)
        };
        for p in [Problem::NotChecked, Problem::OldReading] {
            assert_eq!(
                unplanned(p).as_deref(),
                Some("Press r to check it now."),
                "{p:?}"
            );
        }
        assert_eq!(
            unplanned(Problem::Budget).as_deref(),
            Some("tagteam checks again when the hourly budget allows.")
        );
        for p in [Problem::RateLimited, Problem::Retrying] {
            assert_eq!(unplanned(p), None, "{p:?}");
        }

        // A time that has passed is no time ahead.
        let mut passed = with_problem(Problem::NotChecked);
        passed.usage.retry_at = Some(NOW - 5);
        passed.usage.next_poll_at = Some(NOW);
        assert_eq!(
            remedy(Problem::NotChecked, &passed, &NAMES, NOW).as_deref(),
            Some("Press r to check it now.")
        );
        // With no planned poll, a retry still ahead is the time named.
        let mut retry_only = with_problem(Problem::NotChecked);
        retry_only.usage.next_poll_at = None;
        assert_eq!(
            remedy(Problem::NotChecked, &retry_only, &NAMES, NOW).as_deref(),
            Some("tagteam checks it within 4m.")
        );
        // A row without an email is named by its label.
        let mut token = with_problem(Problem::NoLogin);
        token.row.email = None;
        token.row.label = "a1@x.co".into();
        assert_eq!(
            remedy(Problem::NoLogin, &token, &NAMES, NOW).as_deref(),
            Some("Log in to Claude Code as a1@x.co, then press A here.")
        );
    }
}
