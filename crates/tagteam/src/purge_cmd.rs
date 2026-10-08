//! `tagteam purge` (§10.5): the summary a person confirms, and the result in words or as
//! §13.2's JSON. The engine plans and deletes; this module only renders.

use serde_json::{Value, json};
use tagteam_engine::purge::{PurgePlan, PurgeReport};

/// `n` and the noun, which takes an `s` unless `n` is 1.
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// §10.5 step 2: what the purge deletes, written on stderr before the question. Accounts are
/// grouped by provider when the purge covers several.
pub(crate) fn summary(plan: &PurgePlan) -> String {
    let mut out = String::from("This deletes, for good:\n");
    let mut providers: Vec<&str> = plan.accounts.iter().map(|a| a.provider.as_str()).collect();
    providers.dedup();
    if plan.accounts.is_empty() {
        out.push_str("  no account\n");
    }
    for provider in &providers {
        let indent = if providers.len() > 1 {
            out.push_str(&format!("  {provider}\n"));
            "    "
        } else {
            "  "
        };
        for a in plan
            .accounts
            .iter()
            .filter(|a| a.provider.as_str() == *provider)
        {
            let profile = if a.has_profile {
                ", with its session profile"
            } else {
                ""
            };
            out.push_str(&format!("{indent}#{}  {}{profile}\n", a.position, a.label));
        }
    }
    if !plan.orphan_profiles.is_empty() {
        out.push_str(&format!(
            "  {} that no account owns\n",
            count(plan.orphan_profiles.len(), "session profile")
        ));
    }
    if plan.rescues > 0 {
        out.push_str(&format!(
            "  {} (refreshed tokens not yet in the vault)\n",
            count(plan.rescues, "pending rescue")
        ));
    }
    if plan.displaced > 0 {
        out.push_str(&format!(
            "  {}\n",
            count(plan.displaced, "displaced credential")
        ));
    }
    if plan.store_and_log {
        out.push_str("  the store and the log\n");
    }
    if plan.keychain_orphans {
        out.push_str(
            "  every `tagteam` Keychain item no account names, for every tagteam data directory on this Mac\n",
        );
    }
    out.push_str("It never deletes or changes a live login.\n");
    out
}

/// The question after the summary; the default is no.
pub(crate) const QUESTION: &str = "Delete all of this?";

/// The result in words, on stdout. Failures and warnings go to stderr, apart.
pub(crate) fn human(report: &PurgeReport, full: bool) -> String {
    let mut out = String::new();
    for a in &report.accounts {
        out.push_str(&format!("Purged {} (position {}).\n", a.label, a.position));
    }
    let mut also = Vec::new();
    if report.rescues > 0 {
        also.push(count(report.rescues, "pending rescue"));
    }
    if report.displaced > 0 {
        also.push(count(report.displaced, "displaced credential"));
    }
    if !also.is_empty() {
        out.push_str(&format!("Deleted {}.\n", also.join(" and ")));
    }
    if full && report.store_emptied {
        out.push_str("Emptied the store and deleted the log.\n");
    }
    if out.is_empty() {
        out.push_str("There was nothing to purge.\n");
    }
    out
}

/// §10.5's JSON: `{schemaVersion, ok, provider, accounts: [{number, id, email}], displaced,
/// rescues, storeEmptied, failures: [{what, message}]}`. `email` is the account's label, its
/// email for every provider whose logins have one.
pub(crate) fn json(report: &PurgeReport, provider: Option<&str>) -> Value {
    let accounts: Vec<Value> = report
        .accounts
        .iter()
        .map(|a| json!({"number": a.position, "id": a.id.as_str(), "email": a.label}))
        .collect();
    let failures: Vec<Value> = report
        .failures
        .iter()
        .map(|(what, message)| json!({"what": what, "message": message}))
        .collect();
    json!({
        "schemaVersion": 1,
        "ok": report.failures.is_empty(),
        "provider": provider,
        "accounts": accounts,
        "displaced": report.displaced,
        "rescues": report.rescues,
        "storeEmptied": report.store_emptied,
        "failures": failures,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tagteam_core::{AccountId, ProviderId};
    use tagteam_engine::purge::PurgeAccount;

    use super::*;

    fn account(provider: &str, position: u32, label: &str, has_profile: bool) -> PurgeAccount {
        PurgeAccount {
            id: AccountId::from_string(format!("id-{position}")),
            provider: ProviderId::new(provider),
            position,
            label: label.into(),
            has_profile,
        }
    }

    fn plan() -> PurgePlan {
        PurgePlan {
            provider: None,
            accounts: vec![
                account("claude-code", 1, "a@x.co", true),
                account("claude-code", 2, "b@x.co", false),
            ],
            orphan_profiles: vec![PathBuf::from("/d/sessions/x")],
            rescues: 1,
            displaced: 3,
            store_and_log: true,
            keychain_orphans: false,
        }
    }

    #[test]
    fn the_summary_names_every_account_and_what_else_goes() {
        assert_eq!(
            summary(&plan()),
            "This deletes, for good:\n\
             \x20 #1  a@x.co, with its session profile\n\
             \x20 #2  b@x.co\n\
             \x20 1 session profile that no account owns\n\
             \x20 1 pending rescue (refreshed tokens not yet in the vault)\n\
             \x20 3 displaced credentials\n\
             \x20 the store and the log\n\
             It never deletes or changes a live login.\n"
        );
    }

    #[test]
    fn several_providers_are_grouped_and_keychain_orphans_says_how_far_it_reaches() {
        let p = PurgePlan {
            accounts: vec![
                account("claude-code", 1, "a@x.co", false),
                account("fake-agent", 1, "hank", false),
            ],
            orphan_profiles: vec![],
            rescues: 0,
            displaced: 0,
            keychain_orphans: true,
            ..plan()
        };
        assert_eq!(
            summary(&p),
            "This deletes, for good:\n\
             \x20 claude-code\n\
             \x20   #1  a@x.co\n\
             \x20 fake-agent\n\
             \x20   #1  hank\n\
             \x20 the store and the log\n\
             \x20 every `tagteam` Keychain item no account names, for every tagteam data directory on this Mac\n\
             It never deletes or changes a live login.\n"
        );
        let nothing = PurgePlan {
            provider: Some(ProviderId::new("claude-code")),
            accounts: vec![],
            store_and_log: false,
            keychain_orphans: false,
            ..p
        };
        assert_eq!(
            summary(&nothing),
            "This deletes, for good:\n  no account\nIt never deletes or changes a live login.\n"
        );
    }

    #[test]
    fn the_result_names_each_account_and_the_totals() {
        let report = PurgeReport {
            accounts: plan().accounts,
            displaced: 3,
            rescues: 1,
            store_emptied: true,
            warnings: vec![],
            failures: vec![],
        };
        assert_eq!(
            human(&report, true),
            "Purged a@x.co (position 1).\n\
             Purged b@x.co (position 2).\n\
             Deleted 1 pending rescue and 3 displaced credentials.\n\
             Emptied the store and deleted the log.\n"
        );
        assert_eq!(
            human(&PurgeReport::default(), false),
            "There was nothing to purge.\n"
        );
    }

    #[test]
    fn the_json_is_the_spec_s_shape() {
        let report = PurgeReport {
            accounts: vec![account("claude-code", 2, "b@x.co", false)],
            displaced: 0,
            rescues: 1,
            store_emptied: false,
            warnings: vec!["w".into()],
            failures: vec![("the vault".into(), "locked".into())],
        };
        assert_eq!(
            json(&report, Some("claude-code")),
            json!({"schemaVersion": 1, "ok": false, "provider": "claude-code",
                   "accounts": [{"number": 2, "id": "id-2", "email": "b@x.co"}],
                   "displaced": 0, "rescues": 1, "storeEmptied": false,
                   "failures": [{"what": "the vault", "message": "locked"}]})
        );
        assert_eq!(json(&PurgeReport::default(), None)["provider"], Value::Null);
        assert_eq!(json(&PurgeReport::default(), None)["ok"], true);
    }
}
