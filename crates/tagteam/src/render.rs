use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::SwitchOutcome;
use tagteam_engine::views::{AccountView, ProviderAccounts, StatusView};
use tagteam_provider::SecretStore;

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

/// §13.2: a managed-key account has no usage to fetch, which cswap reports as `api_key`.
fn usage_status(v: &AccountView) -> &'static str {
    if v.kind.managed_key_axis {
        "api_key"
    } else if v.row.quarantine_reason.is_some() {
        "relogin_required"
    } else {
        "unavailable"
    }
}

/// One `list` row (§13.2). Until M2 fetches usage, every row reports no data.
pub fn row_json(v: &AccountView) -> Value {
    let r = &v.row;
    let status = usage_status(v);
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

/// §13.2. `activeAccountNumber` is `provider`'s: the one the command queried, else the default
/// provider (for cswap compatibility), wherever it falls among `lists`.
pub fn list_json(lists: &[ProviderAccounts], provider: &ProviderId) -> Value {
    let active_by: serde_json::Map<String, Value> = lists
        .iter()
        .map(|l| (l.provider.to_string(), json!(l.active_position)))
        .collect();
    let rows: Vec<Value> = lists
        .iter()
        .flat_map(|l| l.accounts.iter().map(row_json))
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
            if let Some(kind) = v.kind.display {
                line.push_str(&format!("  {kind}"));
            }
            if r.disabled {
                line.push_str("  disabled");
            }
            if r.quarantine_reason.is_some() {
                line.push_str("  relogin required");
            }
            s.push_str(&line);
            s.push('\n');
        }
    }
    s
}

/// §13.2. Every shape names `provider` (the one the command ran against) at the top level, and
/// again in `active` wherever there is one: a managed row carries it already.
pub fn status_json(s: &StatusView, provider: &str) -> Value {
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
            let mut row = row_json(account);
            row["managed"] = json!(true);
            json!({"schemaVersion": 1, "provider": provider, "active": row, "totalManagedAccounts": total})
        }
    }
}

pub fn status_human(s: &StatusView) -> String {
    match s {
        StatusView::NoLogin => "No live login.\n".into(),
        StatusView::Unmanaged { email } => format!("Live: {email} (not managed by tagteam)\n"),
        StatusView::Managed { account, total } => {
            let marker = if account.row.quarantine_reason.is_some() {
                ", relogin required"
            } else {
                ""
            };
            format!(
                "Live: {} (position {} of {total}){marker}\n",
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

/// An account command's result: the row as `list` shows it, `active` included.
pub fn account_json(account: &AccountView, created: Option<bool>) -> Value {
    let mut v = json!({"schemaVersion": 1, "ok": true, "account": row_json(account)});
    if let Some(c) = created {
        v["created"] = json!(c);
    }
    v
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tagteam_core::{AccountId, CLAUDE_CODE};
    use tagteam_engine::switch::SwitchReason;
    use tagteam_provider::KindTraits;

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
        let v = list_json(&lists, &ProviderId::new(CLAUDE_CODE));
        assert_eq!(v["activeAccountNumber"], 2);
        assert_eq!(v["activeByProvider"], json!({"other": 5, CLAUDE_CODE: 2}));
        let v = list_json(&lists, &ProviderId::new("other"));
        assert_eq!(v["activeAccountNumber"], 5);
        let v = list_json(&lists, &ProviderId::new("absent"));
        assert_eq!(v["activeAccountNumber"], Value::Null);
    }

    /// `a@x.co`, the live OAuth account at position 1, quarantined or not.
    fn a_view(quarantined: bool) -> AccountView {
        AccountView {
            row: AccountRow {
                id: AccountId::from_string("0192"),
                provider: ProviderId::new(CLAUDE_CODE),
                position: 1,
                identity_key: "a@x.co\n".into(),
                label: "a@x.co".into(),
                email: Some("a@x.co".into()),
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
                quarantine_reason: quarantined.then(|| "invalid_grant".into()),
                quarantine_fp: None,
                quarantine_at: None,
                added_at: 1,
            },
            active: true,
            kind: KindTraits {
                refreshable: true,
                managed_key_axis: false,
                default_email_prefix: None,
                display: None,
            },
        }
    }

    fn one(view: AccountView) -> [ProviderAccounts; 1] {
        [ProviderAccounts {
            provider: ProviderId::new(CLAUDE_CODE),
            active_position: Some(1),
            accounts: vec![view],
        }]
    }

    #[test]
    fn a_quarantined_account_is_marked_for_a_new_login() {
        let names = |id: &str| id.to_owned();
        assert_eq!(
            list_human(&one(a_view(true)), &names),
            "* 1  a@x.co  relogin required\n"
        );
        assert_eq!(list_human(&one(a_view(false)), &names), "* 1  a@x.co\n");
        let status = StatusView::Managed {
            account: a_view(true),
            total: 1,
        };
        assert_eq!(
            status_human(&status),
            "Live: a@x.co (position 1 of 1), relogin required\n"
        );
        let status = StatusView::Managed {
            account: a_view(false),
            total: 1,
        };
        assert_eq!(status_human(&status), "Live: a@x.co (position 1 of 1)\n");
    }
}
