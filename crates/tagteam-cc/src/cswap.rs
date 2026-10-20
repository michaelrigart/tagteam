use serde_json::{Value, json};
use tagteam_core::time::parse_iso8601;
use tagteam_core::{CLAUDE_CODE, ProviderId};
use tagteam_provider::ForeignFormat;
use tagteam_provider::import::{
    FormatError, ImportRecord, accounts, disabled, field_error, optional_string, position,
};

/// Decision 14: a cswap version 1 export (cswap inventory §8.1), as records for Claude Code,
/// the provider cswap drives. `version` must be 1 and `encrypted` false; either is refused by
/// name otherwise.
///
/// Each account maps `number` to the position, `email`, `uuid`, `organizationUuid`,
/// `organizationName` and `config.oauthAccount` to Claude Code's identity payload (§13.3's
/// example), and `credentials` to its credential payload: the credential object, or an
/// `sk-ant-api…` string for an API-key account. cswap marks only API-key accounts
/// (`kind: "api_key"`); its OAuth logins and setup tokens carry no kind, which Claude Code
/// tells apart from the credential (§7.1). An `added` time tagteam cannot read is dropped.
pub fn read_cswap_v1(v: &Value) -> Result<Vec<ImportRecord>, FormatError> {
    match v.get("version").and_then(Value::as_u64) {
        Some(1) => {}
        Some(n) => {
            return Err(FormatError(format!(
                "the cswap export's version is {n}; tagteam reads version 1"
            )));
        }
        None => {
            return Err(FormatError(
                "the cswap export's version is not a number".into(),
            ));
        }
    }
    if v.get("encrypted").and_then(Value::as_bool) == Some(true) {
        return Err(FormatError(
            "the cswap export is encrypted (encrypted: true), which tagteam cannot read".into(),
        ));
    }
    accounts(v, "the cswap export")?
        .into_iter()
        .map(|(i, o)| {
            let email = match o.get("email") {
                Some(Value::String(e)) if !e.is_empty() => e.clone(),
                _ => return Err(field_error(i, "email", "an email address")),
            };
            let oauth_account = match o.get("config").and_then(|c| c.get("oauthAccount")) {
                Some(a @ Value::Object(_)) => a.clone(),
                _ => return Err(field_error(i, "config.oauthAccount", "a JSON object")),
            };
            let credentials = match o.get("credentials") {
                Some(c @ (Value::Object(_) | Value::String(_))) => c.clone(),
                _ => {
                    return Err(field_error(i, "credentials", "a JSON object or an API key"));
                }
            };
            let kind = match optional_string(&o, i, "kind")?.as_deref() {
                None | Some("oauth") => None,
                Some(other) => Some(other.to_owned()),
            };
            let identity = json!({
                "email": email,
                "accountUuid": optional_string(&o, i, "uuid")?,
                "organizationUuid": optional_string(&o, i, "organizationUuid")?,
                "organizationName": optional_string(&o, i, "organizationName")?,
                "oauthAccount": oauth_account,
            });
            Ok(ImportRecord {
                provider: ProviderId::new(CLAUDE_CODE),
                position: position(&o, i, "number")?,
                kind,
                label: Some(email),
                alias: optional_string(&o, i, "alias")?,
                disabled: disabled(&o, i)?,
                added_at: optional_string(&o, i, "added")?
                    .as_deref()
                    .and_then(parse_iso8601)
                    .map(|s| s * 1000),
                identity,
                credential: credentials,
            })
        })
        .collect()
}

fn is_cswap(v: &Value) -> bool {
    v.get("swapVersion").is_some()
}

pub static CSWAP: ForeignFormat = ForeignFormat {
    name: "cswap",
    detect: is_cswap,
    read: read_cswap_v1,
};
