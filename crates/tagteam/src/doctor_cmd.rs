//! `tagteam doctor`'s output (§13.6): its checks grouped per provider, as `list` groups
//! accounts, each with a status glyph and its fix indented beneath, then one summary line; and
//! the `--json` shape.

use serde_json::{Value, json};
use tagteam_core::ProviderId;
use tagteam_engine::doctor::DoctorReport;
use tagteam_provider::CheckStatus;

/// The heading of tagteam's own checks, when the report is headed at all.
const OWN: &str = "tagteam";

fn glyph(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Ok => "✓",
        CheckStatus::Info => "·",
        CheckStatus::Warn => "!",
        CheckStatus::Fail => "✗",
    }
}

/// The report as text. Groups are headed only when more than one provider is in it, as `list`
/// heads its tables: with a single provider in use, the output reads as it would without
/// providers (§13.1). `display_name` names a provider's group.
pub(crate) fn human(report: &DoctorReport, display_name: &dyn Fn(&ProviderId) -> String) -> String {
    let mut groups: Vec<Option<&ProviderId>> = Vec::new();
    for (p, _) in &report.checks {
        if !groups.contains(&p.as_ref()) {
            groups.push(p.as_ref());
        }
    }
    let headed = groups.iter().filter(|g| g.is_some()).count() > 1;
    let indent = if headed { "  " } else { "" };
    let mut s = String::new();
    for group in groups {
        if headed {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(&group.map_or_else(|| OWN.to_owned(), display_name));
            s.push('\n');
        }
        for (_, c) in report.checks.iter().filter(|(p, _)| p.as_ref() == group) {
            s.push_str(&format!("{indent}{} {}\n", glyph(c.status), c.message));
            if let Some(fix) = &c.fix {
                s.push_str(&format!("{indent}    fix: {fix}\n"));
            }
        }
    }
    let count = |status| {
        report
            .checks
            .iter()
            .filter(|(_, c)| c.status == status)
            .count()
    };
    s.push_str(&format!(
        "\n{} ok · {} info · {} warn · {} fail\n",
        count(CheckStatus::Ok),
        count(CheckStatus::Info),
        count(CheckStatus::Warn),
        count(CheckStatus::Fail)
    ));
    s
}

/// §13.6: `{schemaVersion, ok, checks: [{id, provider, status, message, fix}]}`, `provider`
/// null for tagteam's own checks and `fix` null when there is nothing to do.
pub(crate) fn json(report: &DoctorReport) -> Value {
    let checks: Vec<Value> = report
        .checks
        .iter()
        .map(|(p, c)| {
            json!({
                "id": c.id,
                "provider": p.as_ref().map(ProviderId::as_str),
                "status": c.status.as_str(),
                "message": c.message,
                "fix": c.fix,
            })
        })
        .collect();
    json!({"schemaVersion": 1, "ok": report.ok(), "checks": checks})
}

#[cfg(test)]
mod tests {
    use super::*;
    use tagteam_provider::Check;

    fn cc() -> ProviderId {
        ProviderId::new("claude-code")
    }

    fn names(p: &ProviderId) -> String {
        match p.as_str() {
            "claude-code" => "Claude Code".into(),
            other => other.to_owned(),
        }
    }

    fn report(checks: Vec<(Option<ProviderId>, Check)>) -> DoctorReport {
        DoctorReport { checks }
    }

    #[test]
    fn one_provider_reads_as_one_list_with_each_fix_beneath_its_check() {
        let r = report(vec![
            (None, Check::ok("store.integrity", "the store is sound")),
            (
                Some(cc()),
                Check::fail("accounts.vault", "account 2 has no vault entry")
                    .fix("`tagteam remove 2` finishes deleting it"),
            ),
            (
                Some(cc()),
                Check::warn("cc.binary", "`claude` is not on PATH"),
            ),
            (Some(cc()), Check::info("cc.paths", "paths")),
        ]);
        assert_eq!(
            human(&r, &names),
            "✓ the store is sound\n\
             ✗ account 2 has no vault entry\n    fix: `tagteam remove 2` finishes deleting it\n\
             ! `claude` is not on PATH\n\
             · paths\n\
             \n1 ok · 1 info · 1 warn · 1 fail\n"
        );
    }

    #[test]
    fn several_providers_are_headed_by_name_and_tagteam_s_own_checks_by_tagteam() {
        let fake = ProviderId::new("fake-agent");
        let r = report(vec![
            (None, Check::ok("store.integrity", "sound")),
            (Some(cc()), Check::ok("accounts.vault", "cc vault")),
            (
                Some(fake.clone()),
                Check::warn("accounts.vault", "fake vault").fix("do it"),
            ),
        ]);
        assert_eq!(
            human(&r, &names),
            "tagteam\n  ✓ sound\n\
             \nClaude Code\n  ✓ cc vault\n\
             \nfake-agent\n  ! fake vault\n      fix: do it\n\
             \n2 ok · 0 info · 1 warn · 0 fail\n"
        );
    }

    #[test]
    fn the_json_shape_has_null_for_tagteam_s_provider_and_for_no_fix() {
        let r = report(vec![
            (None, Check::info("log.file", "no log yet")),
            (
                Some(cc()),
                Check::fail("accounts.vault", "missing").fix("`tagteam remove 1`"),
            ),
        ]);
        assert_eq!(
            json(&r),
            json!({"schemaVersion": 1, "ok": false, "checks": [
                {"id": "log.file", "provider": null, "status": "info", "message": "no log yet", "fix": null},
                {"id": "accounts.vault", "provider": "claude-code", "status": "fail", "message": "missing", "fix": "`tagteam remove 1`"},
            ]})
        );
        assert_eq!(json(&report(vec![]))["ok"], json!(true));
    }
}
