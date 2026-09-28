use serde_json::{Value, json};
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::SwitchOutcome;
use tagteam_engine::views::{AccountView, ProviderAccounts, StatusView};

const NO_ACCOUNTS: &str = "No accounts yet. Log in with `claude`, then run `tagteam add`.\n";
/// Claude Code reloads a credentials-file change on its next message (Appendix A.3).
const FILE_STORE_HINT: &str = "Active on your next message.";
/// Claude Code caches Keychain reads for 30 s (Appendix A.3).
const KEYCHAIN_HINT: &str = "Claude Code picks this up within about 30 s; restart it to apply now.";

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

fn usage_status(r: &AccountRow) -> &'static str {
    if r.kind == "api_key" {
        "api_key"
    } else if r.quarantine_reason.is_some() {
        "relogin_required"
    } else {
        "unavailable"
    }
}

/// One `list` row (§13.2). Until M2 fetches usage, every row reports no data.
pub fn row_json(v: &AccountView) -> Value {
    let r = &v.row;
    let status = usage_status(r);
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
        "usageStatus": status,
        "usage": null,
        "lastGoodUsage": null,
        "lastGoodFetchedAt": null,
        "lastGoodAgeSeconds": null,
    });
    if status == "unavailable" {
        o["usageError"] = json!("no-data");
        o["usageRetryAt"] = Value::Null;
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

pub fn list_json(lists: &[ProviderAccounts]) -> Value {
    let active_by: serde_json::Map<String, Value> = lists
        .iter()
        .map(|l| (l.provider.to_string(), json!(l.active_position)))
        .collect();
    let rows: Vec<Value> = lists
        .iter()
        .flat_map(|l| l.accounts.iter().map(row_json))
        .collect();
    json!({
        "schemaVersion": 1,
        "activeAccountNumber": lists.first().and_then(|l| l.active_position),
        "activeByProvider": active_by,
        "accounts": rows,
    })
}

pub fn list_human(lists: &[ProviderAccounts], display_names: &dyn Fn(&str) -> String) -> String {
    if lists.iter().all(|l| l.accounts.is_empty()) {
        return NO_ACCOUNTS.into();
    }
    let shown: Vec<&ProviderAccounts> = lists.iter().filter(|l| !l.accounts.is_empty()).collect();
    let mut s = String::new();
    for l in &shown {
        if shown.len() > 1 {
            s.push_str(&format!("{}\n", display_names(l.provider.as_str())));
        }
        for v in &l.accounts {
            let r = &v.row;
            let mut line = format!(
                "{} {}  {}",
                if v.active { "*" } else { " " },
                r.position,
                name(r)
            );
            if let Some(org) = &r.org_name {
                line.push_str(&format!("  [{org}]"));
            }
            match r.kind.as_str() {
                "api_key" => line.push_str("  api key"),
                "setup_token" => line.push_str("  setup token"),
                _ => {}
            }
            if r.disabled {
                line.push_str("  disabled");
            }
            s.push_str(&line);
            s.push('\n');
        }
    }
    s
}

/// §13.2. Every status object names its provider, `provider` (the one the command ran
/// against): a managed row already carries it; an unmanaged login carries it in `active`; with
/// no login there is no `active` object, so it sits at the top level.
pub fn status_json(s: &StatusView, provider: &str) -> Value {
    match s {
        StatusView::NoLogin => {
            json!({"schemaVersion": 1, "provider": provider, "active": null})
        }
        StatusView::Unmanaged { email } => json!({
            "schemaVersion": 1,
            "active": {"email": email, "provider": provider, "managed": false},
        }),
        StatusView::Managed { account, total } => {
            let mut row = row_json(account);
            row["managed"] = json!(true);
            json!({"schemaVersion": 1, "active": row, "totalManagedAccounts": total})
        }
    }
}

pub fn status_human(s: &StatusView) -> String {
    match s {
        StatusView::NoLogin => "No live login.\n".into(),
        StatusView::Unmanaged { email } => format!("Live: {email} (not managed by tagteam)\n"),
        StatusView::Managed { account, total } => {
            format!(
                "Live: {} (position {} of {total})\n",
                name(&account.row),
                account.row.position
            )
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
        "warnings": o.warnings,
    })
}

pub fn switch_human(o: &SwitchOutcome) -> String {
    match (&o.to, o.switched) {
        (Some(to), true) => {
            let hint = if o.file_store {
                FILE_STORE_HINT
            } else {
                KEYCHAIN_HINT
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

pub fn account_json(row: &AccountRow, created: Option<bool>) -> Value {
    let mut v = json!({"schemaVersion": 1, "ok": true, "account": row_json(&AccountView { row: row.clone(), active: false })});
    if let Some(c) = created {
        v["created"] = json!(c);
    }
    v
}
