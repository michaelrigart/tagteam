use serde_json::{Value, json};
use tagteam_core::time::format_iso8601;
use tagteam_core::{AccountId, Pace, ProviderId, Window, WindowKind, printable};
use tagteam_engine::store::{AccountRow, Mapping};
use tagteam_engine::switch::SwitchOutcome;
use tagteam_engine::views::{
    AccountView, ProviderAccounts, ShellAccount, StatusView, UsageStatus, UsageView,
};
use tagteam_engine::words::{
    AHEAD, IN_SESSION, NEEDS_LOGIN, Severity, UPDATED_HEAD, WindowColumn, account_cell, countdown,
    row_notes, severity, spend_text, usage_words, window_columns,
};
// `app` and `transfer_cmd` still spell these `render::email` and `render::name`.
pub use tagteam_engine::words::{email, name};
// `tokens_cmd` still spells these `render::MISSING` and `render::duration`.
pub(crate) use tagteam_engine::words::{MISSING, duration};
use tagteam_provider::SecretStore;
use unicode_width::UnicodeWidthStr;

use crate::SCHEMA_VERSION;

const NO_MAPPINGS: &str = "No mappings yet. Map a directory with `tagteam map ACCOUNT [PATH]`.\n";

/// §13.5's severities, which `list` and `status` share: ≥ 90 critical, ≥ 70 warning.
pub(crate) const CRITICAL: &str = "\x1b[31m";
pub(crate) const WARNING: &str = "\x1b[33m";
pub(crate) const RESET: &str = "\x1b[0m";

/// A row's windows in its provider's JSON shape: `Provider::render_usage` (§13.2).
pub type RenderUsage<'a> = &'a dyn Fn(&ProviderId, &[(Window, Pace)]) -> Value;

/// One `list` row (§13.2). `usage` is decision-grade only (§8.4), with its fetch time and age.
/// Otherwise `usage` is null and the last good reading, if any, is `lastGoodUsage`. Whatever
/// the reading, a row whose status is not `ok` also says why (`usageError`) and when it is
/// retried (`usageRetryAt`, null when it is not, or no longer, ahead). Times are ISO 8601 UTC,
/// as the provider's own `resetsAt`.
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
        }
    }
    if u.status != UsageStatus::Ok {
        o["usageError"] = json!(u.error);
        o["usageRetryAt"] = json!(u.retry_at.map(format_iso8601));
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
    if v.in_session {
        o["inSession"] = json!(true);
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
        "schemaVersion": SCHEMA_VERSION,
        "activeAccountNumber": active,
        "activeByProvider": active_by,
        "accounts": rows,
    })
}

/// §13.5's severities as the ANSI codes `list`, `status`, `auto` and the statusline colour a
/// percentage with: ≥ 90 critical, ≥ 70 warning.
pub(crate) fn severity_code(pct: i64) -> Option<&'static str> {
    match severity(pct) {
        Severity::Critical => Some(CRITICAL),
        Severity::Warning => Some(WARNING),
        Severity::Normal => None,
    }
}

/// The spend window as shown: `€0 of €20`, coloured by the severity of its percentage when
/// `color`; `None` when the provider's detail does not say how much. The one rule `list`,
/// `status` and the statusline share.
pub(crate) fn spend_shown(w: &Window, color: bool) -> Option<String> {
    spend_cell(w, color).map(|c| c.shown)
}

/// `spend_shown` as a table cell, with its plain text for the column's width.
fn spend_cell(w: &Window, color: bool) -> Option<Cell> {
    let text = spend_text(w)?;
    let shown = match severity_code(w.pct.round() as i64) {
        Some(code) if color => format!("{code}{text}{RESET}"),
        _ => text.clone(),
    };
    Some(Cell { plain: text, shown })
}

/// The columns `s` takes on a terminal: a CJK character or an emoji takes two.
pub(crate) fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

pub(crate) fn pad(s: &str, w: usize) -> String {
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
    let shown = match severity_code(n) {
        Some(c) if color => format!("{pad}{c}{digits}{RESET}"),
        _ => format!("{pad}{digits}"),
    };
    Cell {
        plain: format!("{pad}{digits}"),
        shown,
    }
}

/// A window as `list` shows it: its pct (a spend window's amounts), the countdown to its
/// reset, and `▲ fast` when it is ahead of pace.
fn window_cell(w: &Window, p: &Pace, now_s: i64, color: bool) -> Cell {
    let mut c = spend_cell(w, color).unwrap_or_else(|| pct_cell(w.pct, color));
    if let Some(at) = w.resets_at {
        c.push(&format!("  {}", countdown(at, now_s)));
    }
    if p.ahead == Some(true) {
        c.push(&format!("  {AHEAD}"));
    }
    c
}

/// The cell of a row whose reading lacks `col`'s window: `—`, under the `%` of a percentage
/// column and at the start of the spend column.
fn missing_cell(col: &WindowColumn) -> Cell {
    match col.kind {
        WindowKind::Spend => Cell::text(MISSING),
        _ => Cell::text(format!("{MISSING:>4}")),
    }
}

/// One account's line before alignment: its cells (the window columns and UPDATED) when it has a
/// reading, and the notes that follow: its status in words, its kind, `disabled`, `this`.
struct Row {
    marker: char,
    position: u32,
    /// Session-owned (§12.5): `▶` in the session column, which the table has only then.
    session: bool,
    account: String,
    cells: Option<Vec<Cell>>,
    notes: Vec<String>,
}

/// `this`: the row is the run shell's own account (§13.1).
fn row(v: &AccountView, cols: &[WindowColumn], this: bool, now_s: i64, color: bool) -> Row {
    let u = &v.usage;
    let cells = match u.windows.as_deref() {
        Some(ws) if !ws.is_empty() => {
            let mut cells: Vec<Cell> = cols
                .iter()
                .map(|c| {
                    ws.iter()
                        .find(|(w, _)| c.key.as_deref() == Some(w.key.as_str()))
                        .map_or_else(|| missing_cell(c), |(w, p)| window_cell(w, p, now_s, color))
                })
                .collect();
            cells.push(Cell::text(
                u.age_s.map_or_else(|| MISSING.to_owned(), duration),
            ));
            Some(cells)
        }
        _ => None,
    };
    Row {
        marker: if v.active { '*' } else { ' ' },
        position: v.row.position,
        session: v.in_session,
        account: account_cell(&v.row),
        cells,
        notes: row_notes(v, this, now_s),
    }
}

/// §13.1's table for one provider's accounts. `this` is the run shell's own account (§12.8).
fn table(accounts: &[AccountView], this: Option<&AccountId>, now_s: i64, color: bool) -> String {
    let cols = window_columns(accounts);
    let rows: Vec<Row> = accounts
        .iter()
        .map(|v| row(v, &cols, this == Some(&v.row.id), now_s, color))
        .collect();
    // Decision 11: the session column exists only while some row is session-owned, so every
    // other table keeps today's layout byte for byte.
    let sessions = rows.iter().any(|r| r.session);
    let account_w = rows
        .iter()
        .map(|r| width(&r.account))
        .fold(width("ACCOUNT"), usize::max);
    let heads: Vec<&str> = cols
        .iter()
        .map(|c| c.head.as_str())
        .chain((!cols.is_empty()).then_some(UPDATED_HEAD))
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
    let gap = if sessions { "    " } else { "  " };
    let mut header = format!("    #{gap}{}", pad("ACCOUNT", account_w));
    for (head, w) in heads.iter().zip(&widths) {
        header.push_str("  ");
        header.push_str(&pad(head, *w));
    }
    let mut out = format!("{}\n", header.trim_end());
    for r in &rows {
        let session = match (sessions, r.session) {
            (false, _) => String::new(),
            (true, true) => format!(" {IN_SESSION}"),
            (true, false) => "  ".to_owned(),
        };
        let mut line = format!(
            " {} {:>2}{session}  {}",
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

fn no_accounts(launch: &str) -> String {
    format!("No accounts yet. Log in with `{launch}`, then run `tagteam add`.\n")
}

/// §13.1's `list`: one table per provider that has accounts, headed by its name when there
/// is more than one. `this` is the run shell's own account (§12.8), noted `this`; `now_s`
/// measures the countdowns; `color` colours percentages.
pub fn list_human(
    lists: &[ProviderAccounts],
    this: Option<&AccountId>,
    display_names: &dyn Fn(&str) -> String,
    launch: &str,
    now_s: i64,
    color: bool,
) -> String {
    if lists.iter().all(|l| l.accounts.is_empty()) {
        return no_accounts(launch);
    }
    let shown: Vec<&ProviderAccounts> = lists.iter().filter(|l| !l.accounts.is_empty()).collect();
    let mut s = String::new();
    for l in &shown {
        if shown.len() > 1 {
            s.push_str(&format!("{}\n", display_names(l.provider.as_str())));
        }
        s.push_str(&table(&l.accounts, this, now_s, color));
    }
    s
}

/// §13.2. Every shape names `provider` (the one the command ran against) at the top level, and
/// again in `active` wherever there is one: a managed row carries it already. In a run shell
/// (§12.8) each shape also names the session's account, last: `session: {number, position,
/// id, email}`, or `null` for an account tagteam does not manage. Outside one there is no
/// `session` key at all.
pub fn status_json(
    s: &StatusView,
    session: &ShellAccount,
    provider: &str,
    usage: RenderUsage<'_>,
) -> Value {
    let mut v = match s {
        StatusView::NoLogin => {
            json!({"schemaVersion": SCHEMA_VERSION, "provider": provider, "active": null})
        }
        StatusView::Unmanaged { email } => json!({
            "schemaVersion": SCHEMA_VERSION,
            "provider": provider,
            "active": {"email": email, "provider": provider, "managed": false},
        }),
        StatusView::Managed { account, total } => {
            let mut row = row_json(account, usage);
            row["managed"] = json!(true);
            json!({"schemaVersion": SCHEMA_VERSION, "provider": provider, "active": row, "totalManagedAccounts": total})
        }
    };
    match session {
        ShellAccount::NotInShell => {}
        ShellAccount::Managed(row) => {
            v["session"] = json!({
                "number": row.position,
                "position": row.position,
                "id": row.id.as_str(),
                "email": email(row),
            });
        }
        ShellAccount::Unmanaged => v["session"] = Value::Null,
    }
    v
}

/// `status`'s usage line: each window of the reading with its countdown and pace, the
/// reading's age, and a status other than `ok` in words. A quarantined account without a
/// reading has none: the line above already says it.
fn usage_line(u: &UsageView, now_s: i64, color: bool) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(ws) = u.windows.as_deref().filter(|ws| !ws.is_empty()) {
        for (w, p) in ws {
            let shown = match spend_cell(w, color) {
                Some(cell) => cell.shown,
                None => pct_cell(w.pct, color).shown.trim_start().to_owned(),
            };
            let mut part = format!("{} {shown}", w.label);
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
        parts.push(usage_words(u, now_s));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// `status` (§13.1): the live login and its usage, and in a run shell (§12.8) the session's own
/// account on a last line. `session_login` names an unmanaged session's login as its profile
/// holds it; it is read only for `ShellAccount::Unmanaged`.
pub fn status_human(
    s: &StatusView,
    session: &ShellAccount,
    session_login: Option<&str>,
    now_s: i64,
    color: bool,
) -> String {
    let mut out = match s {
        StatusView::NoLogin => "No live login.\n".to_owned(),
        StatusView::Unmanaged { email } => {
            format!("Live: {} (not managed by tagteam)\n", printable(email))
        }
        StatusView::Managed { account, total } => {
            let marker = if account.row.quarantine_reason.is_some() {
                format!(", {NEEDS_LOGIN}")
            } else {
                String::new()
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
    };
    match session {
        ShellAccount::NotInShell => {}
        ShellAccount::Managed(row) => out.push_str(&format!(
            "This session: {} (position {})\n",
            name(row),
            row.position
        )),
        ShellAccount::Unmanaged => match session_login {
            Some(login) => out.push_str(&format!(
                "This session: {} (not managed by tagteam)\n",
                printable(login)
            )),
            None => out.push_str("This session: not managed by tagteam\n"),
        },
    }
    out
}

pub fn switch_json(o: &SwitchOutcome, provider: &str) -> Value {
    json!({
        "schemaVersion": SCHEMA_VERSION,
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
            let hint = o
                .activation_hint
                .map(|h| format!("{h}\n"))
                .unwrap_or_default();
            format!(
                "Switched to {} (position {}).\n{hint}",
                name(to),
                to.position
            )
        }
        _ => format!("{}\n", o.message),
    }
}

/// An account command's result: the row as `list` shows it, `active` and usage included.
pub fn account_json(account: &AccountView, created: Option<bool>, usage: RenderUsage<'_>) -> Value {
    let mut v =
        json!({"schemaVersion": SCHEMA_VERSION, "ok": true, "account": row_json(account, usage)});
    if let Some(c) = created {
        v["created"] = json!(c);
    }
    v
}

/// One mapping (§12.7) and the account it names, as `map` reports it. `addedAt` is ISO 8601
/// UTC, as `history`'s times are.
pub fn mapping_json(m: &Mapping, account: &AccountRow) -> Value {
    let mut o = json!({
        "path": m.path,
        "provider": m.provider.as_str(),
        "number": account.position,
        "id": account.id.as_str(),
        "email": email(account),
        "addedAt": format_iso8601(m.added_at.div_euclid(1000)),
    });
    if let Some(a) = &account.alias {
        o["alias"] = json!(a);
    }
    o
}

/// `map`'s list.
pub fn mappings_json(rows: &[(Mapping, AccountRow)]) -> Value {
    let mappings: Vec<Value> = rows.iter().map(|(m, a)| mapping_json(m, a)).collect();
    json!({"schemaVersion": SCHEMA_VERSION, "mappings": mappings})
}

/// `map`'s list in text: one line per mapping, in path order. The provider shows only when the
/// mappings span more than one, so a single provider looks as it would without providers
/// (§13.1).
pub fn mappings_human(rows: &[(Mapping, AccountRow)]) -> String {
    let Some((first, _)) = rows.first() else {
        return NO_MAPPINGS.to_owned();
    };
    let several = rows.iter().any(|(m, _)| m.provider != first.provider);
    rows.iter()
        .map(|(m, a)| {
            if several {
                format!("{}  {}  {}  {}\n", m.path, m.provider, a.position, name(a))
            } else {
                format!("{}  {}  {}\n", m.path, a.position, name(a))
            }
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod testutil {
    use tagteam_cc::CLAUDE_CODE;
    use tagteam_core::AccountId;
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
            in_session: false,
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
            next_poll_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tagteam_cc::CLAUDE_CODE;
    use tagteam_engine::switch::{LiveLogin, SwitchReason};
    use tagteam_engine::views::{NO_DATA, forecast_of};

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
            managed_key_replaced: false,
            activation_hint: None,
        }
    }

    #[test]
    fn an_unmanaged_login_is_printable_in_status() {
        let live = StatusView::Unmanaged {
            email: "a\u{1b}[31m@x.co".into(),
        };
        assert_eq!(
            status_human(&live, &ShellAccount::NotInShell, None, NOW, false),
            "Live: a\u{fffd}[31m@x.co (not managed by tagteam)\n"
        );
        let human = status_human(
            &live,
            &ShellAccount::Unmanaged,
            Some("b\u{9b}@x.co"),
            NOW,
            false,
        );
        assert_eq!(
            human.lines().last(),
            Some("This session: b\u{fffd}@x.co (not managed by tagteam)")
        );
        let json = status_json(&live, &ShellAccount::NotInShell, CLAUDE_CODE, &count);
        assert_eq!(json["active"]["email"], "a\u{1b}[31m@x.co");
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

    /// `row` of `kind`, as a switch's side.
    fn side(position: u32, kind: &str) -> AccountRow {
        let mut row = view(
            position,
            "x@x.co",
            OAUTH,
            unread(UsageStatus::Unavailable, None, None),
        )
        .row;
        row.kind = kind.into();
        row
    }

    fn switched(from: Option<&str>, to: &str, stored_in: SecretStore) -> SwitchOutcome {
        SwitchOutcome {
            from: from.map(|k| side(1, k)),
            to: Some(side(2, to)),
            stored_in: Some(stored_in),
            ..stored(None)
        }
    }

    #[test]
    fn switch_human_prints_the_engine_s_hint_under_the_line() {
        let hinted = SwitchOutcome {
            activation_hint: Some("Hint."),
            ..switched(Some("oauth"), "oauth", SecretStore::Keychain)
        };
        assert_eq!(
            switch_human(&hinted),
            "Switched to x@x.co (position 2).\nHint.\n"
        );
        let bare = switched(Some("oauth"), "oauth", SecretStore::Keychain);
        assert_eq!(switch_human(&bare), "Switched to x@x.co (position 2).\n");
    }

    /// A renderer that only says how many windows it was handed.
    fn count(_: &ProviderId, windows: &[(Window, Pace)]) -> Value {
        json!({"windows": windows.len()})
    }

    fn accounts(provider: &str, active: Option<u32>) -> ProviderAccounts {
        let live = active.map_or(LiveLogin::None, |position| LiveLogin::Managed { position });
        ProviderAccounts {
            provider: ProviderId::new(provider),
            active_position: active,
            accounts: vec![],
            threshold: 90.0,
            strategy: tagteam_core::autoswitch::AutoStrategy::Best,
            names: tagteam_engine::views::ProviderNames {
                display_name: "Claude Code",
                launch_command: "claude",
            },
            models: vec![],
            forecast: forecast_of(&[], &live, &[]),
            live,
        }
    }

    #[test]
    fn the_empty_list_names_the_provider_s_launch_command() {
        let names = |id: &str| id.to_owned();
        assert_eq!(
            list_human(
                &[accounts(CLAUDE_CODE, None)],
                None,
                &names,
                "claude",
                NOW,
                false
            ),
            "No accounts yet. Log in with `claude`, then run `tagteam add`.\n"
        );
        assert_eq!(
            list_human(
                &[accounts("fake-agent", None)],
                None,
                &names,
                "fakeagent",
                NOW,
                false
            ),
            "No accounts yet. Log in with `fakeagent`, then run `tagteam add`.\n"
        );
    }

    #[test]
    fn a_usage_strategy_outcome_renders_its_strategy_reason_and_warnings() {
        let message =
            "every candidate is at its limit: a@x.co (7d at 100%); the earliest reset is in 3d09h";
        let o = SwitchOutcome {
            switched: false,
            strategy: "next-available",
            reason: SwitchReason::CandidatesExhausted,
            message: message.into(),
            warnings: vec!["usage was not collected: the store is locked".into()],
            ..stored(None)
        };
        assert_eq!(
            switch_json(&o, CLAUDE_CODE),
            json!({"schemaVersion": 1, "provider": CLAUDE_CODE, "switched": false, "from": null,
                   "to": null, "strategy": "next-available", "reason": "candidates-exhausted",
                   "message": message, "credentialStore": null,
                   "warnings": ["usage was not collected: the store is locked"]})
        );
        assert_eq!(switch_human(&o), format!("{message}\n"));
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
        let live = accounts
            .iter()
            .find(|v| v.active)
            .map_or(LiveLogin::None, |v| LiveLogin::Managed {
                position: v.row.position,
            });
        [ProviderAccounts {
            provider: ProviderId::new(CLAUDE_CODE),
            active_position: None,
            forecast: forecast_of(&accounts, &live, &[]),
            accounts,
            threshold: 90.0,
            strategy: tagteam_core::autoswitch::AutoStrategy::Best,
            names: tagteam_engine::views::ProviderNames {
                display_name: "Claude Code",
                launch_command: "claude",
            },
            models: vec![],
            live,
        }]
    }

    fn names(id: &str) -> String {
        id.to_owned()
    }

    #[test]
    fn severity_codes_follow_the_shared_severities() {
        // `words::severity` decides; the CLI only maps it to §13.5's colours.
        for (pct, code) in [
            (0, None),
            (69, None),
            (70, Some(WARNING)),
            (89, Some(WARNING)),
            (90, Some(CRITICAL)),
            (100, Some(CRITICAL)),
        ] {
            assert_eq!(severity_code(pct), code, "{pct}");
        }
    }

    #[test]
    fn the_list_is_a_table_of_windows_countdowns_pace_and_age() {
        // §13.1's layout: a column per window some reading has (SPEND always), `▲ fast`, the
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
            list_human(
                &one(vec![live, spare, work, key]),
                None,
                &names,
                "claude",
                NOW,
                false
            ),
            concat!(
                "    #  ACCOUNT                5H           7D                   SPEND      FABLE        UPDATED\n",
                " *  1  michael@example.com      9%  2h40m   77%  3d09h  ▲ fast  €0 of €20    0%  3d09h  2m\n",
                "    2  spare@example.com       31%  2h40m   12%  3d09h          —             —         14m\n",
                "    3  work (w@corp.com)      needs a new login\n",
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
            list_human(&one(rows), None, &names, "claude", NOW, false),
            concat!(
                "    #  ACCOUNT                    5H           7D           SPEND  UPDATED\n",
                "    1  a@x.co                      50%  reset   60%  3d09h  —      1h06m    rate-limited (retry 5m)\n",
                "    2  b@x.co                     not checked yet\n",
                "    3  c@x.co                     paused to stay within the request budget (retry 10m)\n",
                "    4  setup-token-4@token.local  usage check failed (pre-send, retry <1m)  setup token  disabled\n",
            )
        );
    }

    #[test]
    fn a_row_with_a_reading_shows_the_status_words_beside_it() {
        let mut locked = read(900, 31.0, 12.0, false, vec![]);
        locked.status = UsageStatus::KeychainUnavailable;
        locked.retry_at = Some(NOW + 120);
        assert_eq!(
            list_human(
                &one(vec![view(1, "a@x.co", OAUTH, locked)]),
                None,
                &names,
                "claude",
                NOW,
                false
            ),
            concat!(
                "    #  ACCOUNT  5H           7D           SPEND  UPDATED\n",
                "    1  a@x.co    31%  2h40m   12%  3d09h  —      15m      can't read the Keychain (retry 2m)\n",
            )
        );
    }

    #[test]
    fn the_spend_amount_is_coloured_by_its_severity_in_list_and_status() {
        let rows = |pct: f64| {
            let mut spent = spend(pct * 0.2, 20.0);
            spent.pct = pct;
            view(1, "a@x.co", OAUTH, read(0, 9.0, 12.0, false, vec![spent]))
        };
        let table =
            |pct, color| list_human(&one(vec![rows(pct)]), None, &names, "claude", NOW, color);
        assert!(table(95.0, true).contains("\x1b[31m€19 of €20\x1b[0m"));
        assert!(table(75.0, true).contains("\x1b[33m€15 of €20\x1b[0m"));
        assert!(table(10.0, true).contains("  €2 of €20"));
        assert!(!table(10.0, true).contains("\x1b[31m€2"));
        assert!(!table(95.0, false).contains('\x1b'), "never without colour");
        let status = |pct, color| {
            let account = rows(pct);
            status_human(
                &StatusView::Managed { account, total: 1 },
                &ShellAccount::NotInShell,
                None,
                NOW,
                color,
            )
        };
        assert!(status(95.0, true).contains("spend \x1b[31m€19 of €20\x1b[0m"));
        assert!(status(95.0, false).contains("spend €19 of €20"));
        // The column still lines up: colour codes take no width.
        let plain = table(95.0, false);
        let coloured = table(95.0, true);
        let strip = |s: &str| s.replace("\x1b[31m", "").replace("\x1b[0m", "");
        assert_eq!(strip(&coloured), plain);
    }

    #[test]
    fn wide_characters_take_two_columns_so_the_table_still_lines_up() {
        let live =
            |position, email: &str| view(position, email, OAUTH, read(0, 9.0, 12.0, false, vec![]));
        let out = list_human(
            &one(vec![
                live(1, "a@x.co"),
                live(2, "山田太郎@x.co"),
                live(3, "😀@x.co"),
            ]),
            None,
            &names,
            "claude",
            NOW,
            false,
        );
        assert_eq!(
            out,
            concat!(
                "    #  ACCOUNT        5H           7D           SPEND  UPDATED\n",
                "    1  a@x.co           9%  2h40m   12%  3d09h  —      <1m\n",
                "    2  山田太郎@x.co    9%  2h40m   12%  3d09h  —      <1m\n",
                "    3  😀@x.co          9%  2h40m   12%  3d09h  —      <1m\n",
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
            list_human(&rows(), None, &names, "claude", NOW, true),
            concat!(
                "    #  ACCOUNT  5H           7D           SPEND  UPDATED\n",
                "    1  a@x.co    \x1b[31m95%\x1b[0m  2h40m   \x1b[33m77%\x1b[0m  3d09h  —      <1m\n",
            )
        );
        assert!(!list_human(&rows(), None, &names, "claude", NOW, false).contains('\x1b'));
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
                &ShellAccount::NotInShell,
                None,
                NOW,
                false
            ),
            "Live: a@x.co (position 1 of 1)\n  5h 9% (2h40m) · 7d 77% (3d09h) ▲ fast · spend €0 of €20 · Fable 0% (3d09h) · 2m old\n"
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
                &ShellAccount::NotInShell,
                None,
                NOW,
                false
            ),
            "Live: a@x.co (position 1 of 2)\n  usage check failed (pre-send, retry <1m)\n"
        );
        assert_eq!(
            status_human(
                &StatusView::NoLogin,
                &ShellAccount::NotInShell,
                None,
                NOW,
                false
            ),
            "No live login.\n"
        );
        assert_eq!(
            status_human(
                &StatusView::Unmanaged {
                    email: "s@x.co".into()
                },
                &ShellAccount::NotInShell,
                None,
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
            list_human(&one(vec![a.clone()]), None, &names, "claude", NOW, false),
            "    #  ACCOUNT\n *  1  a@x.co   needs a new login\n"
        );
        assert_eq!(
            status_human(
                &StatusView::Managed {
                    account: a,
                    total: 1
                },
                &ShellAccount::NotInShell,
                None,
                NOW,
                false
            ),
            "Live: a@x.co (position 1 of 1), needs a new login\n"
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

        // A status that is not `ok` says why and when whatever the reading: a decision-grade
        // one is shown, and the error beside it is not hidden.
        let mut locked = read(120, 9.0, 77.0, true, vec![]);
        locked.status = UsageStatus::TokenExpired;
        locked.error = Some("token-expired".into());
        locked.retry_at = Some(NOW + 330);
        let beside = row_json(&view(1, "a@x.co", OAUTH, locked), &count);
        assert_eq!(
            keys(&beside),
            [
                &ALWAYS[..],
                &[
                    "usageFetchedAt",
                    "usageAgeSeconds",
                    "usageError",
                    "usageRetryAt"
                ]
            ]
            .concat()
        );
        assert_eq!(
            (
                &beside["usageStatus"],
                &beside["usage"],
                &beside["usageError"]
            ),
            (
                &json!("token_expired"),
                &json!({"windows": 2}),
                &json!("token-expired")
            )
        );
        assert_eq!(beside["usageRetryAt"], json!(format_iso8601(NOW + 330)));
        // A passed or absent retry is null, not missing.
        let mut due = read(120, 9.0, 77.0, true, vec![]);
        due.status = UsageStatus::KeychainUnavailable;
        due.error = Some("keychain-unavailable".into());
        let due = row_json(&view(1, "a@x.co", OAUTH, due), &count);
        assert_eq!(
            (&due["usageError"], &due["usageRetryAt"]),
            (&json!("keychain-unavailable"), &Value::Null)
        );

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
        assert_eq!(
            keys(&key),
            [&ALWAYS[..], &last_good, &["usageError", "usageRetryAt"]].concat()
        );
        assert_eq!(key["usageStatus"], "api_key");
        assert_eq!(
            (&key["usageError"], &key["usageRetryAt"]),
            (&Value::Null, &Value::Null)
        );
    }

    /// The rows of `the_list_is_a_table_of_windows_countdowns_pace_and_age`, none of them in a
    /// session, and the table they make (`PINNED`).
    fn pinned_rows() -> Vec<AccountView> {
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
        vec![live, spare, work, key]
    }

    const PINNED: &str = concat!(
        "    #  ACCOUNT                5H           7D                   SPEND      FABLE        UPDATED\n",
        " *  1  michael@example.com      9%  2h40m   77%  3d09h  ▲ fast  €0 of €20    0%  3d09h  2m\n",
        "    2  spare@example.com       31%  2h40m   12%  3d09h          —             —         14m\n",
        "    3  work (w@corp.com)      needs a new login\n",
        "    4  api-key-4@token.local  api key\n",
    );

    #[test]
    fn without_a_session_owned_row_the_table_is_byte_identical() {
        // Decision 11: the column exists only while some row is session-owned. A run shell
        // whose own account is not listed (one tagteam does not manage) leaves it as it was.
        let elsewhere = AccountId::from_string("0192-unmanaged");
        for this in [None, Some(&elsewhere)] {
            assert_eq!(
                list_human(&one(pinned_rows()), this, &names, "claude", NOW, false),
                PINNED
            );
        }
    }

    #[test]
    fn a_session_owned_row_is_marked_after_its_position_and_the_run_shells_own_is_noted() {
        // §13.1's `▶`, in a one-column field after the position, with the header moved to
        // match; the run shell's own account gets the trailing note `this` (Decision 11).
        let mut rows = pinned_rows();
        rows[1].in_session = true;
        let spare = rows[1].row.id.clone();
        assert_eq!(
            list_human(
                &one(rows.clone()),
                Some(&spare),
                &names,
                "claude",
                NOW,
                false
            ),
            concat!(
                "    #    ACCOUNT                5H           7D                   SPEND      FABLE        UPDATED\n",
                " *  1    michael@example.com      9%  2h40m   77%  3d09h  ▲ fast  €0 of €20    0%  3d09h  2m\n",
                "    2 ▶  spare@example.com       31%  2h40m   12%  3d09h          —             —         14m      this\n",
                "    3    work (w@corp.com)      needs a new login\n",
                "    4    api-key-4@token.local  api key\n",
            )
        );
        // Outside a run shell the same session shows, without the note.
        assert_eq!(
            list_human(&one(rows), None, &names, "claude", NOW, false),
            concat!(
                "    #    ACCOUNT                5H           7D                   SPEND      FABLE        UPDATED\n",
                " *  1    michael@example.com      9%  2h40m   77%  3d09h  ▲ fast  €0 of €20    0%  3d09h  2m\n",
                "    2 ▶  spare@example.com       31%  2h40m   12%  3d09h          —             —         14m\n",
                "    3    work (w@corp.com)      needs a new login\n",
                "    4    api-key-4@token.local  api key\n",
            )
        );
    }

    #[test]
    fn row_json_marks_a_session_owned_row_last_and_only_when_it_is() {
        // §13.2: `alias?, disabled?: true, loginExpiresAt?, inSession?: true`.
        let mut v = view(1, "a@x.co", OAUTH, read(120, 9.0, 77.0, true, vec![]));
        v.row.alias = Some("w".into());
        v.row.disabled = true;
        v.row.login_expires_at = Some(1_797_000_000_000);
        let tail = [
            "usageFetchedAt",
            "usageAgeSeconds",
            "alias",
            "disabled",
            "loginExpiresAt",
        ];
        assert_eq!(keys(&row_json(&v, &count)), [&ALWAYS[..], &tail].concat());
        v.in_session = true;
        let row = row_json(&v, &count);
        assert_eq!(keys(&row), [&ALWAYS[..], &tail, &["inSession"]].concat());
        assert_eq!(row["inSession"], json!(true));
        assert_eq!(
            account_json(&v, None, &count)["account"]["inSession"],
            json!(true),
            "an account command's row too"
        );
    }

    #[test]
    fn status_names_the_run_shells_account_in_every_shape_and_only_in_a_run_shell() {
        let mut side = view(3, "s@x.co", OAUTH, unread(UsageStatus::Ok, None, None));
        side.row.alias = Some("side".into());
        let session = ShellAccount::Managed(side.row.clone());
        let named = json!({"number": 3, "position": 3, "id": "id-3", "email": "s@x.co"});
        let mut live = view(1, "a@x.co", OAUTH, read(120, 9.0, 77.0, true, vec![]));
        live.active = true;
        let shapes = [
            StatusView::NoLogin,
            StatusView::Unmanaged {
                email: "u@x.co".into(),
            },
            StatusView::Managed {
                account: live,
                total: 3,
            },
        ];
        for shape in &shapes {
            let outside = status_json(shape, &ShellAccount::NotInShell, CLAUDE_CODE, &count);
            assert!(outside.get("session").is_none(), "{outside}");
            let inside = status_json(shape, &session, CLAUDE_CODE, &count);
            assert_eq!(keys(&inside).last(), Some(&"session"), "{inside}");
            assert_eq!(inside["session"], named);
            let unmanaged = status_json(shape, &ShellAccount::Unmanaged, CLAUDE_CODE, &count);
            assert_eq!(unmanaged.get("session"), Some(&Value::Null));
            // Everything else is as it is outside a run shell.
            let mut rest = inside.clone();
            rest.as_object_mut().unwrap().remove("session");
            assert_eq!((keys(&rest), &rest), (keys(&outside), &outside));
        }
        assert_eq!(
            status_json(
                &StatusView::NoLogin,
                &ShellAccount::NotInShell,
                CLAUDE_CODE,
                &count
            ),
            json!({"schemaVersion": 1, "provider": CLAUDE_CODE, "active": null})
        );
    }

    #[test]
    fn status_says_which_account_this_session_runs() {
        let mut side = view(3, "w@corp.com", OAUTH, unread(UsageStatus::Ok, None, None));
        side.row.alias = Some("work".into());
        let session = ShellAccount::Managed(side.row.clone());
        let failing = view(
            1,
            "a@x.co",
            OAUTH,
            unread(UsageStatus::Unavailable, Some("pre-send"), Some(30)),
        );
        let managed = StatusView::Managed {
            account: failing,
            total: 3,
        };
        assert_eq!(
            status_human(&managed, &session, None, NOW, false),
            "Live: a@x.co (position 1 of 3)\n  usage check failed (pre-send, retry <1m)\nThis session: work (w@corp.com) (position 3)\n"
        );
        assert_eq!(
            status_human(&StatusView::NoLogin, &session, None, NOW, false),
            "No live login.\nThis session: work (w@corp.com) (position 3)\n"
        );
        let stranger = StatusView::Unmanaged {
            email: "u@x.co".into(),
        };
        assert_eq!(
            status_human(
                &stranger,
                &ShellAccount::Unmanaged,
                Some("c@x.co"),
                NOW,
                false
            ),
            "Live: u@x.co (not managed by tagteam)\nThis session: c@x.co (not managed by tagteam)\n"
        );
        assert_eq!(
            status_human(
                &StatusView::NoLogin,
                &ShellAccount::Unmanaged,
                None,
                NOW,
                false
            ),
            "No live login.\nThis session: not managed by tagteam\n"
        );
        assert_eq!(
            status_human(
                &managed,
                &ShellAccount::NotInShell,
                Some("ignored"),
                NOW,
                false
            ),
            "Live: a@x.co (position 1 of 3)\n  usage check failed (pre-send, retry <1m)\n",
            "outside a run shell, as before"
        );
    }

    #[test]
    fn a_mapping_line_names_the_provider_only_when_several_are_mapped() {
        let row = |position, email: &str| {
            view(position, email, OAUTH, unread(UsageStatus::Ok, None, None)).row
        };
        let mapping = |path: &str, account: &AccountRow| Mapping {
            path: path.into(),
            provider: account.provider.clone(),
            account_id: account.id.clone(),
            added_at: NOW * 1000,
        };
        let (a, b) = (row(1, "a@x.co"), row(2, "b@x.co"));
        let one = [
            (mapping("/w", &a), a.clone()),
            (mapping("/x", &b), b.clone()),
        ];
        assert_eq!(mappings_human(&one), "/w  1  a@x.co\n/x  2  b@x.co\n");
        let mut f = row(1, "f@x.co");
        f.provider = ProviderId::new("fake-agent");
        let two = [
            (mapping("/w", &a), a.clone()),
            (mapping("/w", &f), f.clone()),
        ];
        assert_eq!(
            mappings_human(&two),
            "/w  claude-code  1  a@x.co\n/w  fake-agent  1  f@x.co\n"
        );
        assert_eq!(mappings_human(&[]), NO_MAPPINGS);
    }

    #[test]
    fn a_mapping_s_json_names_its_account_and_when_it_was_made() {
        let mut a = view(1, "a@x.co", OAUTH, unread(UsageStatus::Ok, None, None)).row;
        let m = Mapping {
            path: "/w".into(),
            provider: a.provider.clone(),
            account_id: a.id.clone(),
            added_at: NOW * 1000 + 999,
        };
        let expected = json!({"path": "/w", "provider": "claude-code", "number": 1,
                              "id": "id-1", "email": "a@x.co", "addedAt": "2026-09-21T14:13:20Z"});
        assert_eq!(mapping_json(&m, &a), expected);
        a.alias = Some("work".into());
        let mut aliased = expected.clone();
        aliased["alias"] = json!("work");
        assert_eq!(mapping_json(&m, &a), aliased);
        assert_eq!(
            mappings_json(&[(m, a)]),
            json!({"schemaVersion": 1, "mappings": [aliased]})
        );
    }
}
