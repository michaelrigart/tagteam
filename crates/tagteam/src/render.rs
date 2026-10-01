use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_core::{Pace, ProviderId, Window, WindowKind};
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::SwitchOutcome;
use tagteam_engine::views::{
    AccountView, NO_DATA, ProviderAccounts, StatusView, UsageStatus, UsageView,
};
use tagteam_provider::SecretStore;

const NO_ACCOUNTS: &str = "No accounts yet. Log in with `claude`, then run `tagteam add`.\n";
/// Claude Code reloads a credentials-file change on its next message (Appendix A.3).
const FILE_STORE_HINT: &str = "Active on your next message.";
/// Claude Code caches Keychain reads for 30 s (Appendix A.3).
const KEYCHAIN_HINT: &str = "Claude Code picks this up within about 30 s; restart it to apply now.";

/// §13.5's severities, which `list` and `status` share: ≥ 90 critical, ≥ 70 warning.
pub(crate) const CRITICAL: &str = "\x1b[31m";
pub(crate) const WARNING: &str = "\x1b[33m";
pub(crate) const RESET: &str = "\x1b[0m";
/// A window a reading does not have, or an age that is not known.
pub(crate) const MISSING: &str = "—";
/// §8.7's marker for a window ahead of pace.
const AHEAD: &str = "▲ pace";
/// `list` always has a spend column once any account has a reading (§13.1).
const SPEND_HEAD: &str = "SPEND";

/// A row's windows in its provider's JSON shape: `Provider::render_usage` (§13.2).
pub type RenderUsage<'a> = &'a dyn Fn(&ProviderId, &[(Window, Pace)]) -> Value;

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

/// One `list` row (§13.2). `usage` is decision-grade only (§8.4), with its fetch time and age.
/// Otherwise `usage` is null and the last good reading, if any, is `lastGoodUsage`; an
/// `unavailable` row also says why and when it is retried. Times are ISO 8601 UTC, as the
/// provider's own `resetsAt`.
pub fn row_json(v: &AccountView, usage: RenderUsage<'_>) -> Value {
    let r = &v.row;
    let u = &v.usage;
    let mut o = json!({
        "number": r.position,
        "position": r.position,
        "id": r.id.as_str(),
        "provider": r.provider.as_str(),
        "email": email(r),
        "organizationName": r.org_name,
        "organizationUuid": r.org_uuid,
        "isOrganization": !r.org_uuid.is_empty(),
        "active": v.active,
        "usageStatus": u.status.as_str(),
    });
    let rendered = u.windows.as_deref().map(|w| usage(&r.provider, w));
    let fetched_at = u.fetched_at.map(format_iso8601);
    match rendered {
        Some(current) if u.decision_grade => {
            o["usage"] = current;
            o["usageFetchedAt"] = json!(fetched_at);
            o["usageAgeSeconds"] = json!(u.age_s);
        }
        last_good => {
            o["usage"] = Value::Null;
            o["lastGoodUsage"] = last_good.unwrap_or(Value::Null);
            o["lastGoodFetchedAt"] = json!(fetched_at);
            o["lastGoodAgeSeconds"] = json!(u.age_s);
            if u.status == UsageStatus::Unavailable {
                o["usageError"] = json!(u.error);
                o["usageRetryAt"] = json!(u.retry_at.map(format_iso8601));
            }
        }
    }
    if let Some(a) = &r.alias {
        o["alias"] = json!(a);
    }
    if r.disabled {
        o["disabled"] = json!(true);
    }
    if let Some(e) = r.login_expires_at {
        o["loginExpiresAt"] = json!(e);
    }
    o
}

/// §13.2. `activeAccountNumber` is `provider`'s: the one the command queried, else the default
/// provider (for cswap compatibility), wherever it falls among `lists`.
pub fn list_json(
    lists: &[ProviderAccounts],
    provider: &ProviderId,
    usage: RenderUsage<'_>,
) -> Value {
    let active_by: serde_json::Map<String, Value> = lists
        .iter()
        .map(|l| (l.provider.to_string(), json!(l.active_position)))
        .collect();
    let rows: Vec<Value> = lists
        .iter()
        .flat_map(|l| l.accounts.iter().map(|v| row_json(v, usage)))
        .collect();
    let active = lists
        .iter()
        .find(|l| &l.provider == provider)
        .and_then(|l| l.active_position);
    json!({
        "schemaVersion": 1,
        "activeAccountNumber": active,
        "activeByProvider": active_by,
        "accounts": rows,
    })
}

/// A span of time as `list` and `status` show it: `3d09h`, `2h40m`, `45m`, or `<1m`.
pub(crate) fn duration(secs: i64) -> String {
    let s = secs.max(0);
    let (days, hours, minutes) = (s / 86_400, s % 86_400 / 3_600, s % 3_600 / 60);
    if days > 0 {
        format!("{days}d{hours:02}h")
    } else if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "<1m".into()
    }
}

/// The time left until `at`, or `reset` once it has passed: the reading predates the window's
/// reset, so its pct no longer applies.
fn countdown(at: i64, now_s: i64) -> String {
    if at <= now_s {
        "reset".into()
    } else {
        duration(at - now_s)
    }
}

pub(crate) fn severity(pct: i64) -> Option<&'static str> {
    if pct >= 90 {
        Some(CRITICAL)
    } else if pct >= 70 {
        Some(WARNING)
    } else {
        None
    }
}

/// An amount with its currency's symbol (€, $, £), or its code for any other; a whole amount
/// without cents.
pub(crate) fn money(amount: f64, currency: &str) -> String {
    let n = if amount.fract() == 0.0 {
        format!("{amount:.0}")
    } else {
        format!("{amount:.2}")
    };
    match currency.to_ascii_uppercase().as_str() {
        "EUR" => format!("€{n}"),
        "USD" => format!("${n}"),
        "GBP" => format!("£{n}"),
        code => format!("{n} {code}"),
    }
}

/// `€0 of €20`, from a spend window's `detail {used, limit, currency}` (§8.2).
fn spend_text(w: &Window) -> Option<String> {
    let d = w.detail.as_ref().filter(|_| w.kind == WindowKind::Spend)?;
    let currency = d["currency"].as_str()?;
    Some(format!(
        "{} of {}",
        money(d["used"].as_f64()?, currency),
        money(d["limit"].as_f64()?, currency)
    ))
}

fn width(s: &str) -> usize {
    s.chars().count()
}

fn pad(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(width(s))))
}

/// A table cell: its text, and the text as printed, with its percentage perhaps coloured.
struct Cell {
    plain: String,
    shown: String,
}

impl Cell {
    fn text(s: impl Into<String>) -> Self {
        let plain = s.into();
        Cell {
            shown: plain.clone(),
            plain,
        }
    }

    fn push(&mut self, s: &str) {
        self.plain.push_str(s);
        self.shown.push_str(s);
    }

    fn width(&self) -> usize {
        width(&self.plain)
    }
}

/// A pct, rounded and right-aligned in four columns, coloured by severity when `color`.
fn pct_cell(pct: f64, color: bool) -> Cell {
    let n = pct.round() as i64;
    let digits = format!("{n}%");
    let pad = " ".repeat(4usize.saturating_sub(digits.len()));
    let shown = match severity(n) {
        Some(c) if color => format!("{pad}{c}{digits}{RESET}"),
        _ => format!("{pad}{digits}"),
    };
    Cell {
        plain: format!("{pad}{digits}"),
        shown,
    }
}

/// A window as `list` shows it: its pct (a spend window's amounts), the countdown to its
/// reset, and `▲ pace` when it is ahead of pace.
fn window_cell(w: &Window, p: &Pace, now_s: i64, color: bool) -> Cell {
    let mut c = match spend_text(w) {
        Some(t) => Cell::text(t),
        None => pct_cell(w.pct, color),
    };
    if let Some(at) = w.resets_at {
        c.push(&format!("  {}", countdown(at, now_s)));
    }
    if p.ahead == Some(true) {
        c.push(&format!("  {AHEAD}"));
    }
    c
}

/// A usage status in words (§13.1): in place of a row's windows when it has no reading, and
/// beside them when its status is not `ok`.
fn words(u: &UsageView, now_s: i64) -> String {
    let retry = u
        .retry_at
        .filter(|&at| at > now_s)
        .map(|at| format!("retry {}", duration(at - now_s)));
    match u.status {
        UsageStatus::Ok => "no usage reported".into(),
        UsageStatus::TokenExpired => "token expired".into(),
        UsageStatus::ApiKey => "api key".into(),
        UsageStatus::KeychainUnavailable => "keychain unavailable".into(),
        UsageStatus::ReloginRequired => "relogin required".into(),
        UsageStatus::ForeignCredential => "foreign credential".into(),
        UsageStatus::NoCredentials => "no credentials".into(),
        UsageStatus::Unsupported => "usage unsupported".into(),
        UsageStatus::Unavailable => match (u.error.as_deref(), retry) {
            (None | Some(NO_DATA), _) => "no data yet".into(),
            (Some("over-budget"), Some(r)) => format!("over budget ({r})"),
            (Some("over-budget"), None) => "over budget".into(),
            (Some(e), Some(r)) => format!("unavailable ({e}, {r})"),
            (Some(e), None) => format!("unavailable ({e})"),
        },
    }
}

/// One of `list`'s window columns: the window key it shows (`None` for the spend column when
/// no account has spend), headed by the window's label in capitals.
struct Column {
    key: Option<String>,
    kind: WindowKind,
    head: String,
}

impl Column {
    /// The cell of a row whose reading lacks this window: `—`, under the `%` of a percentage
    /// column and at the start of the spend column.
    fn missing(&self) -> Cell {
        match self.kind {
            WindowKind::Spend => Cell::text(MISSING),
            _ => Cell::text(format!("{MISSING:>4}")),
        }
    }
}

/// §13.1's window columns for one provider's accounts: every window key some reading has, in
/// kind order (Short, Long, Spend, Scoped) and then by first appearance. SPEND is always a
/// column, `—` throughout when no account has spend: only scoped windows come and go. No
/// columns at all while no account has a reading.
fn columns(accounts: &[AccountView]) -> Vec<Column> {
    let windows: Vec<&Window> = accounts
        .iter()
        .filter_map(|v| v.usage.windows.as_ref())
        .flatten()
        .map(|(w, _)| w)
        .collect();
    let mut cols: Vec<Column> = Vec::new();
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
                cols.push(Column {
                    key: Some(w.key.clone()),
                    kind,
                    head: w.label.to_uppercase(),
                });
            }
        }
        if kind == WindowKind::Spend && !windows.iter().any(|w| w.kind == WindowKind::Spend) {
            cols.push(Column {
                key: None,
                kind,
                head: SPEND_HEAD.into(),
            });
        }
    }
    cols
}

/// One account's line before alignment: its cells (the window columns and AGE) when it has a
/// reading, and the notes that follow: its status in words, its kind, `disabled`.
struct Row {
    marker: char,
    position: u32,
    account: String,
    cells: Option<Vec<Cell>>,
    notes: Vec<String>,
}

fn row(v: &AccountView, cols: &[Column], now_s: i64, color: bool) -> Row {
    let u = &v.usage;
    let mut notes = Vec::new();
    let cells = match u.windows.as_deref() {
        Some(ws) if !ws.is_empty() => {
            let mut cells: Vec<Cell> = cols
                .iter()
                .map(|c| {
                    ws.iter()
                        .find(|(w, _)| c.key.as_deref() == Some(w.key.as_str()))
                        .map_or_else(|| c.missing(), |(w, p)| window_cell(w, p, now_s, color))
                })
                .collect();
            cells.push(Cell::text(
                u.age_s.map_or_else(|| MISSING.to_owned(), duration),
            ));
            if u.status != UsageStatus::Ok {
                notes.push(words(u, now_s));
            }
            Some(cells)
        }
        _ => {
            notes.push(words(u, now_s));
            None
        }
    };
    if let Some(kind) = v.kind.display {
        if !notes.iter().any(|n| n == kind) {
            notes.push(kind.to_owned());
        }
    }
    if v.row.disabled {
        notes.push("disabled".into());
    }
    let account = match &v.row.org_name {
        Some(org) => format!("{} [{org}]", name(&v.row)),
        None => name(&v.row),
    };
    Row {
        marker: if v.active { '*' } else { ' ' },
        position: v.row.position,
        account,
        cells,
        notes,
    }
}

/// §13.1's table for one provider's accounts.
fn table(accounts: &[AccountView], now_s: i64, color: bool) -> String {
    let cols = columns(accounts);
    let rows: Vec<Row> = accounts
        .iter()
        .map(|v| row(v, &cols, now_s, color))
        .collect();
    let account_w = rows
        .iter()
        .map(|r| width(&r.account))
        .fold(width("ACCOUNT"), usize::max);
    let heads: Vec<&str> = cols
        .iter()
        .map(|c| c.head.as_str())
        .chain((!cols.is_empty()).then_some("AGE"))
        .collect();
    let widths: Vec<usize> = heads
        .iter()
        .enumerate()
        .map(|(i, head)| {
            rows.iter()
                .filter_map(|r| r.cells.as_ref())
                .map(|cells| cells[i].width())
                .fold(width(head), usize::max)
        })
        .collect();
    let mut header = format!("    #  {}", pad("ACCOUNT", account_w));
    for (head, w) in heads.iter().zip(&widths) {
        header.push_str("  ");
        header.push_str(&pad(head, *w));
    }
    let mut out = format!("{}\n", header.trim_end());
    for r in &rows {
        let mut line = format!(
            " {} {:>2}  {}",
            r.marker,
            r.position,
            pad(&r.account, account_w)
        );
        for (c, w) in r.cells.iter().flatten().zip(&widths) {
            line.push_str("  ");
            line.push_str(&c.shown);
            line.push_str(&" ".repeat(w.saturating_sub(c.width())));
        }
        for n in &r.notes {
            line.push_str("  ");
            line.push_str(n);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// §13.1's `list`: one table per provider that has accounts, headed by its name when there
/// is more than one. `now_s` measures the countdowns; `color` colours percentages.
pub fn list_human(
    lists: &[ProviderAccounts],
    display_names: &dyn Fn(&str) -> String,
    now_s: i64,
    color: bool,
) -> String {
    if lists.iter().all(|l| l.accounts.is_empty()) {
        return NO_ACCOUNTS.into();
    }
    let shown: Vec<&ProviderAccounts> = lists.iter().filter(|l| !l.accounts.is_empty()).collect();
    let mut s = String::new();
    for l in &shown {
        if shown.len() > 1 {
            s.push_str(&format!("{}\n", display_names(l.provider.as_str())));
        }
        s.push_str(&table(&l.accounts, now_s, color));
    }
    s
}

/// §13.2. Every shape names `provider` (the one the command ran against) at the top level, and
/// again in `active` wherever there is one: a managed row carries it already.
pub fn status_json(s: &StatusView, provider: &str, usage: RenderUsage<'_>) -> Value {
    match s {
        StatusView::NoLogin => {
            json!({"schemaVersion": 1, "provider": provider, "active": null})
        }
        StatusView::Unmanaged { email } => json!({
            "schemaVersion": 1,
            "provider": provider,
            "active": {"email": email, "provider": provider, "managed": false},
        }),
        StatusView::Managed { account, total } => {
            let mut row = row_json(account, usage);
            row["managed"] = json!(true);
            json!({"schemaVersion": 1, "provider": provider, "active": row, "totalManagedAccounts": total})
        }
    }
}

/// `status`'s usage line: each window of the reading with its countdown and pace, the
/// reading's age, and a status other than `ok` in words. A quarantined account without a
/// reading has none: the line above already says it.
fn usage_line(u: &UsageView, now_s: i64, color: bool) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(ws) = u.windows.as_deref().filter(|ws| !ws.is_empty()) {
        for (w, p) in ws {
            let mut part = match spend_text(w) {
                Some(t) => format!("{} {t}", w.label),
                None => format!("{} {}", w.label, pct_cell(w.pct, color).shown.trim_start()),
            };
            if let Some(at) = w.resets_at {
                part.push_str(&format!(" ({})", countdown(at, now_s)));
            }
            if p.ahead == Some(true) {
                part.push_str(&format!(" {AHEAD}"));
            }
            parts.push(part);
        }
        parts.push(format!(
            "{} old",
            u.age_s.map_or_else(|| MISSING.to_owned(), duration)
        ));
    }
    if u.status != UsageStatus::Ok && u.status != UsageStatus::ReloginRequired {
        parts.push(words(u, now_s));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

pub fn status_human(s: &StatusView, now_s: i64, color: bool) -> String {
    match s {
        StatusView::NoLogin => "No live login.\n".into(),
        StatusView::Unmanaged { email } => format!("Live: {email} (not managed by tagteam)\n"),
        StatusView::Managed { account, total } => {
            let marker = if account.row.quarantine_reason.is_some() {
                ", relogin required"
            } else {
                ""
            };
            let mut out = format!(
                "Live: {} (position {} of {total}){marker}\n",
                name(&account.row),
                account.row.position
            );
            if let Some(line) = usage_line(&account.usage, now_s, color) {
                out.push_str(&format!("  {line}\n"));
            }
            out
        }
    }
}

pub fn switch_json(o: &SwitchOutcome, provider: &str) -> Value {
    json!({
        "schemaVersion": 1,
        "provider": provider,
        "switched": o.switched,
        "from": o.from.as_ref().map(|r| r.position),
        "to": o.to.as_ref().map(|r| r.position),
        "strategy": o.strategy,
        "reason": o.reason.as_str(),
        "message": o.message,
        "credentialStore": credential_store(o),
        "warnings": o.warnings,
    })
}

/// §13.2: where the switch stored the credential, as the engine reports it; `None` when it
/// wrote none (a no-op).
fn credential_store(o: &SwitchOutcome) -> Option<&'static str> {
    o.stored_in.as_ref().map(|s| match s {
        SecretStore::Keychain => "keychain",
        SecretStore::File(_) | SecretStore::Fallback(_) => "file",
    })
}

/// The stderr notice for a write the keychain refused, naming where the secret went.
pub fn fallback_notice(o: &SwitchOutcome) -> Option<String> {
    match &o.stored_in {
        Some(SecretStore::Fallback(path)) => Some(format!(
            "the Keychain refused the write, so the credential was stored in {} instead",
            path.display()
        )),
        _ => None,
    }
}

pub fn switch_human(o: &SwitchOutcome) -> String {
    match (&o.to, o.switched) {
        (Some(to), true) => {
            let hint = match o.stored_in {
                Some(SecretStore::File(_) | SecretStore::Fallback(_)) => FILE_STORE_HINT,
                Some(SecretStore::Keychain) | None => KEYCHAIN_HINT,
            };
            format!(
                "Switched to {} (position {}).\n{hint}\n",
                name(to),
                to.position
            )
        }
        _ => format!("{}\n", o.message),
    }
}

/// An account command's result: the row as `list` shows it, `active` and usage included.
pub fn account_json(account: &AccountView, created: Option<bool>, usage: RenderUsage<'_>) -> Value {
    let mut v = json!({"schemaVersion": 1, "ok": true, "account": row_json(account, usage)});
    if let Some(c) = created {
        v["created"] = json!(c);
    }
    v
}

#[cfg(test)]
pub(crate) mod testutil {
    use tagteam_core::{AccountId, CLAUDE_CODE};
    use tagteam_provider::KindTraits;

    use super::*;

    /// Any fixed instant: every reading and reset below is placed relative to it.
    pub(crate) const NOW: i64 = 1_790_000_000;

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

    pub(crate) const SETUP_TOKEN: KindTraits = KindTraits {
        refreshable: false,
        managed_key_axis: false,
        default_email_prefix: Some("setup-token"),
        display: Some("setup token"),
    };

    /// `email`'s inactive row at `position`, of `kind`, with `usage`.
    pub(crate) fn view(
        position: u32,
        email: &str,
        kind: KindTraits,
        usage: UsageView,
    ) -> AccountView {
        AccountView {
            row: AccountRow {
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
            },
            active: false,
            kind,
            usage,
        }
    }

    /// No reading: `status`, with `error` and a retry `retry_in` seconds from now.
    pub(crate) fn unread(
        status: UsageStatus,
        error: Option<&str>,
        retry_in: Option<i64>,
    ) -> UsageView {
        UsageView {
            status,
            windows: None,
            decision_grade: false,
            fetched_at: None,
            age_s: None,
            error: error.map(str::to_owned),
            retry_at: retry_in.map(|s| NOW + s),
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
            resets_at: resets_in.map(|s| NOW + s),
            period_s: None,
            detail: None,
        }
    }

    /// Claude Code's spend window: `used` of `limit` euros.
    pub(crate) fn spend(used: f64, limit: f64) -> Window {
        Window {
            detail: Some(json!({"used": used, "limit": limit, "currency": "EUR"})),
            ..window(
                "spend",
                "spend",
                WindowKind::Spend,
                used / limit * 100.0,
                None,
            )
        }
    }

    pub(crate) fn fable(pct: f64) -> Window {
        window(
            "scoped:Fable",
            "Fable",
            WindowKind::Scoped,
            pct,
            Some(291_630),
        )
    }

    /// A good reading `age_s` old: 5h resetting in 2h40m30s and 7d in 3d09h00m30s (ahead of pace
    /// when `ahead`), then `extra`. Decision-grade while at most 300 s old.
    pub(crate) fn read(
        age_s: i64,
        five: f64,
        seven: f64,
        ahead: bool,
        extra: Vec<Window>,
    ) -> UsageView {
        let mut windows = vec![
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
        ];
        windows.extend(extra.into_iter().map(|w| (w, Pace::default())));
        UsageView {
            status: UsageStatus::Ok,
            windows: Some(windows),
            decision_grade: age_s <= 300,
            fetched_at: Some(NOW - age_s),
            age_s: Some(age_s),
            error: None,
            retry_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tagteam_core::CLAUDE_CODE;
    use tagteam_engine::switch::SwitchReason;

    use super::testutil::*;
    use super::*;

    fn stored(stored_in: Option<SecretStore>) -> SwitchOutcome {
        SwitchOutcome {
            switched: true,
            from: None,
            to: None,
            strategy: "direct",
            reason: SwitchReason::Switched,
            message: String::new(),
            warnings: vec![],
            stored_in,
            unmanaged_email: None,
        }
    }

    #[test]
    fn only_a_fallback_is_a_notice_and_either_file_is_a_file() {
        let path = PathBuf::from("/home/u/.claude.json");
        let linux = stored(Some(SecretStore::File(path.clone())));
        assert_eq!(
            (credential_store(&linux), fallback_notice(&linux)),
            (Some("file"), None)
        );
        let fell_back = stored(Some(SecretStore::Fallback(path)));
        assert_eq!(credential_store(&fell_back), Some("file"));
        assert_eq!(
            fallback_notice(&fell_back).as_deref(),
            Some(
                "the Keychain refused the write, so the credential was stored in /home/u/.claude.json instead"
            )
        );
        let keychain = stored(Some(SecretStore::Keychain));
        assert_eq!(
            (credential_store(&keychain), fallback_notice(&keychain)),
            (Some("keychain"), None)
        );
        let none = stored(None);
        assert_eq!(
            (credential_store(&none), fallback_notice(&none)),
            (None, None)
        );
    }

    /// A renderer that only says how many windows it was handed.
    fn count(_: &ProviderId, windows: &[(Window, Pace)]) -> Value {
        json!({"windows": windows.len()})
    }

    fn accounts(provider: &str, active: Option<u32>) -> ProviderAccounts {
        ProviderAccounts {
            provider: ProviderId::new(provider),
            active_position: active,
            accounts: vec![],
        }
    }

    #[test]
    fn active_account_number_is_the_queried_providers_wherever_it_is_listed() {
        let lists = [accounts("other", Some(5)), accounts(CLAUDE_CODE, Some(2))];
        let v = list_json(&lists, &ProviderId::new(CLAUDE_CODE), &count);
        assert_eq!(v["activeAccountNumber"], 2);
        assert_eq!(v["activeByProvider"], json!({"other": 5, CLAUDE_CODE: 2}));
        let v = list_json(&lists, &ProviderId::new("other"), &count);
        assert_eq!(v["activeAccountNumber"], 5);
        let v = list_json(&lists, &ProviderId::new("absent"), &count);
        assert_eq!(v["activeAccountNumber"], Value::Null);
    }

    fn one(accounts: Vec<AccountView>) -> [ProviderAccounts; 1] {
        [ProviderAccounts {
            provider: ProviderId::new(CLAUDE_CODE),
            active_position: None,
            accounts,
        }]
    }

    fn names(id: &str) -> String {
        id.to_owned()
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
        assert_eq!(countdown(NOW + 60, NOW), "1m");
        assert_eq!(countdown(NOW, NOW), "reset");
    }

    #[test]
    fn money_uses_the_symbol_or_the_code() {
        assert_eq!(money(0.0, "EUR"), "€0");
        assert_eq!(money(20.0, "eur"), "€20");
        assert_eq!(money(12.5, "USD"), "$12.50");
        assert_eq!(money(3.0, "GBP"), "£3");
        assert_eq!(money(7.25, "CHF"), "7.25 CHF");
    }

    #[test]
    fn the_list_is_a_table_of_windows_countdowns_pace_and_age() {
        // §13.1's layout: a column per window some reading has (SPEND always), `▲ pace`, the
        // reading's age, and the status in words for a row with no reading.
        let mut live = view(
            1,
            "michael@example.com",
            OAUTH,
            read(120, 9.0, 77.0, true, vec![spend(0.0, 20.0), fable(0.0)]),
        );
        live.active = true;
        let spare = view(
            2,
            "spare@example.com",
            OAUTH,
            read(840, 31.0, 12.0, false, vec![]),
        );
        let mut work = view(
            3,
            "w@corp.com",
            OAUTH,
            unread(UsageStatus::ReloginRequired, None, None),
        );
        work.row.alias = Some("work".into());
        work.row.quarantine_reason = Some("invalid_grant".into());
        let key = view(
            4,
            "api-key-4@token.local",
            API_KEY,
            unread(UsageStatus::ApiKey, None, None),
        );
        assert_eq!(
            list_human(&one(vec![live, spare, work, key]), &names, NOW, false),
            concat!(
                "    #  ACCOUNT                5H           7D                   SPEND      FABLE        AGE\n",
                " *  1  michael@example.com      9%  2h40m   77%  3d09h  ▲ pace  €0 of €20    0%  3d09h  2m\n",
                "    2  spare@example.com       31%  2h40m   12%  3d09h          —             —         14m\n",
                "    3  work (w@corp.com)      relogin required\n",
                "    4  api-key-4@token.local  api key\n",
            )
        );
    }

    #[test]
    fn a_row_says_in_words_what_keeps_its_reading_from_being_current() {
        // A stale reading is still shown, with its age and why it is not refreshed; a window whose
        // reset has passed says so. Rows with no reading say why instead of the columns.
        let mut stale = read(4_000, 50.0, 60.0, false, vec![]);
        stale.status = UsageStatus::Unavailable;
        stale.error = Some("http-429".into());
        stale.retry_at = Some(NOW + 330);
        stale.windows.as_mut().unwrap()[0].0.resets_at = Some(NOW - 10);
        let fresh = unread(UsageStatus::Unavailable, Some(NO_DATA), None);
        let over = unread(UsageStatus::Unavailable, Some("over-budget"), Some(600));
        let mut setup = view(
            4,
            "setup-token-4@token.local",
            SETUP_TOKEN,
            unread(UsageStatus::Unavailable, Some("pre-send"), Some(30)),
        );
        setup.row.disabled = true;
        let rows = vec![
            view(1, "a@x.co", OAUTH, stale),
            view(2, "b@x.co", OAUTH, fresh),
            view(3, "c@x.co", OAUTH, over),
            setup,
        ];
        assert_eq!(
            list_human(&one(rows), &names, NOW, false),
            concat!(
                "    #  ACCOUNT                    5H           7D           SPEND  AGE\n",
                "    1  a@x.co                      50%  reset   60%  3d09h  —      1h06m  unavailable (http-429, retry 5m)\n",
                "    2  b@x.co                     no data yet\n",
                "    3  c@x.co                     over budget (retry 10m)\n",
                "    4  setup-token-4@token.local  unavailable (pre-send, retry <1m)  setup token  disabled\n",
            )
        );
    }

    #[test]
    fn percentages_are_coloured_by_severity_only_when_asked() {
        // §13.5's severities: ≥ 90 red, ≥ 70 yellow, and the columns still line up.
        let rows = || {
            one(vec![view(
                1,
                "a@x.co",
                OAUTH,
                read(0, 95.0, 77.0, false, vec![]),
            )])
        };
        assert_eq!(
            list_human(&rows(), &names, NOW, true),
            concat!(
                "    #  ACCOUNT  5H           7D           SPEND  AGE\n",
                "    1  a@x.co    \x1b[31m95%\x1b[0m  2h40m   \x1b[33m77%\x1b[0m  3d09h  —      <1m\n",
            )
        );
        assert!(!list_human(&rows(), &names, NOW, false).contains('\x1b'));
    }

    #[test]
    fn status_shows_the_reading_or_the_trouble_on_a_second_line() {
        let mut live = view(
            1,
            "a@x.co",
            OAUTH,
            read(120, 9.0, 77.0, true, vec![spend(0.0, 20.0), fable(0.0)]),
        );
        live.active = true;
        assert_eq!(
            status_human(
                &StatusView::Managed {
                    account: live,
                    total: 1
                },
                NOW,
                false
            ),
            "Live: a@x.co (position 1 of 1)\n  5h 9% (2h40m) · 7d 77% (3d09h) ▲ pace · spend €0 of €20 · Fable 0% (3d09h) · 2m old\n"
        );
        let failing = view(
            1,
            "a@x.co",
            OAUTH,
            unread(UsageStatus::Unavailable, Some("pre-send"), Some(30)),
        );
        assert_eq!(
            status_human(
                &StatusView::Managed {
                    account: failing,
                    total: 2
                },
                NOW,
                false
            ),
            "Live: a@x.co (position 1 of 2)\n  unavailable (pre-send, retry <1m)\n"
        );
        assert_eq!(
            status_human(&StatusView::NoLogin, NOW, false),
            "No live login.\n"
        );
        assert_eq!(
            status_human(
                &StatusView::Unmanaged {
                    email: "s@x.co".into()
                },
                NOW,
                false
            ),
            "Live: s@x.co (not managed by tagteam)\n"
        );
    }

    #[test]
    fn a_quarantined_account_is_marked_for_a_new_login() {
        let mut a = view(
            1,
            "a@x.co",
            OAUTH,
            unread(UsageStatus::ReloginRequired, None, None),
        );
        a.active = true;
        a.row.quarantine_reason = Some("invalid_grant".into());
        assert_eq!(
            list_human(&one(vec![a.clone()]), &names, NOW, false),
            "    #  ACCOUNT\n *  1  a@x.co   relogin required\n"
        );
        assert_eq!(
            status_human(
                &StatusView::Managed {
                    account: a,
                    total: 1
                },
                NOW,
                false
            ),
            "Live: a@x.co (position 1 of 1), relogin required\n"
        );
    }

    /// The row's keys, in order.
    fn keys(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    const ALWAYS: [&str; 11] = [
        "number",
        "position",
        "id",
        "provider",
        "email",
        "organizationName",
        "organizationUuid",
        "isOrganization",
        "active",
        "usageStatus",
        "usage",
    ];

    #[test]
    fn row_json_carries_usage_only_when_decision_grade() {
        // §13.2: `usage` and its fetch time and age when it is decision-grade; otherwise the last
        // good reading's, and why and when for an `unavailable` row.
        let current = row_json(
            &view(1, "a@x.co", OAUTH, read(120, 9.0, 77.0, true, vec![])),
            &count,
        );
        assert_eq!(
            keys(&current),
            [&ALWAYS[..], &["usageFetchedAt", "usageAgeSeconds"]].concat()
        );
        assert_eq!(
            (&current["usageStatus"], &current["usage"]),
            (&json!("ok"), &json!({"windows": 2}))
        );
        assert_eq!(
            (&current["usageFetchedAt"], &current["usageAgeSeconds"]),
            (&json!(format_iso8601(NOW - 120)), &json!(120))
        );

        let stale = row_json(
            &view(1, "a@x.co", OAUTH, read(840, 9.0, 77.0, true, vec![])),
            &count,
        );
        let last_good = ["lastGoodUsage", "lastGoodFetchedAt", "lastGoodAgeSeconds"];
        assert_eq!(keys(&stale), [&ALWAYS[..], &last_good].concat());
        assert_eq!(
            (
                &stale["usage"],
                &stale["lastGoodUsage"],
                &stale["lastGoodAgeSeconds"]
            ),
            (&Value::Null, &json!({"windows": 2}), &json!(840))
        );
        assert_eq!(stale["lastGoodFetchedAt"], json!(format_iso8601(NOW - 840)));

        let failing = row_json(
            &view(
                1,
                "a@x.co",
                OAUTH,
                unread(UsageStatus::Unavailable, Some("http-429"), Some(330)),
            ),
            &count,
        );
        assert_eq!(
            keys(&failing),
            [&ALWAYS[..], &last_good, &["usageError", "usageRetryAt"]].concat()
        );
        assert_eq!(
            (
                &failing["usageStatus"],
                &failing["lastGoodUsage"],
                &failing["lastGoodFetchedAt"]
            ),
            (&json!("unavailable"), &Value::Null, &Value::Null)
        );
        assert_eq!(
            (&failing["usageError"], &failing["usageRetryAt"]),
            (&json!("http-429"), &json!(format_iso8601(NOW + 330)))
        );

        let key = row_json(
            &view(
                2,
                "k@x.co",
                API_KEY,
                unread(UsageStatus::ApiKey, None, None),
            ),
            &count,
        );
        assert_eq!(keys(&key), [&ALWAYS[..], &last_good].concat());
        assert_eq!(key["usageStatus"], "api_key");
    }
}
