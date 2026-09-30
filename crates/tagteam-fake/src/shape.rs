use serde_json::{Map, Value};
use tagteam_core::Fingerprint;
use tagteam_provider::{Identity, KindTraits, ProviderError};

/// Refreshable: carries a `renew` token.
pub const KIND_TOKEN: &str = "fa_token";
/// Not refreshable, and on the same (only) axis as `fa_token`.
pub const KIND_STATIC: &str = "fa_static";
pub const KINDS: [&str; 2] = [KIND_TOKEN, KIND_STATIC];
/// FakeAgent's only machine-shared credential key.
pub const DEVICE: &str = "device";

fn fa(bytes: &[u8]) -> Option<Map<String, Value>> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    v.get("fa")?.as_object().cloned()
}

fn field<'a>(o: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    o.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

pub(crate) fn token(bytes: &[u8]) -> Option<String> {
    fa(bytes).and_then(|o| field(&o, "token").map(str::to_owned))
}

pub(crate) fn renew(bytes: &[u8]) -> Option<String> {
    fa(bytes).and_then(|o| field(&o, "renew").map(str::to_owned))
}

pub(crate) fn expires(bytes: &[u8]) -> Option<i64> {
    fa(bytes)?.get("expires")?.as_i64()
}

pub(crate) fn classify(bytes: &[u8]) -> &'static str {
    if renew(bytes).is_some() {
        KIND_TOKEN
    } else {
        KIND_STATIC
    }
}

/// §2 "Generation": the renew token, else the access token.
pub(crate) fn fingerprint(bytes: &[u8]) -> Option<Fingerprint> {
    renew(bytes)
        .or_else(|| token(bytes))
        .map(|s| Fingerprint::of_secret(s.as_bytes()))
}

/// Both tokens empty: FakeAgent's own logged-out state.
pub(crate) fn is_wiped(bytes: &[u8]) -> bool {
    fa(bytes).is_some_and(|o| field(&o, "token").is_none() && field(&o, "renew").is_none())
}

pub(crate) fn kind_traits(kind: &str) -> KindTraits {
    let plain = KindTraits {
        refreshable: false,
        managed_key_axis: false,
        default_email_prefix: None,
        display: None,
    };
    match kind {
        KIND_TOKEN => KindTraits {
            refreshable: true,
            ..plain
        },
        KIND_STATIC => KindTraits {
            default_email_prefix: Some("fa-static"),
            display: Some("static"),
            ..plain
        },
        _ => plain,
    }
}

/// `None` when `raw` names no handle: no login.
pub(crate) fn identity_from(raw: &Value) -> Option<Identity> {
    let handle = raw
        .get("handle")?
        .as_str()
        .filter(|s| !s.is_empty())?
        .to_owned();
    let workspace = raw
        .get("workspace")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let uid = raw
        .get("uid")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let label = if workspace.is_empty() {
        handle
    } else {
        format!("{handle}@{workspace}")
    };
    Some(Identity {
        label,
        email: None,
        org_uuid: workspace,
        org_name: None,
        account_uuid: uid,
        raw: raw.clone(),
    })
}

/// Account-scoped keys from the target, the machine-shared `device` key from the live
/// credential, absence included.
pub(crate) fn compose(
    target: &[u8],
    live: Option<&Map<String, Value>>,
) -> Result<Vec<u8>, ProviderError> {
    let mut out = match serde_json::from_slice::<Value>(target) {
        Ok(Value::Object(o)) => o,
        _ => {
            return Err(ProviderError::Invalid(
                "the stored FakeAgent credential is not a JSON object".into(),
            ));
        }
    };
    out.shift_remove(DEVICE);
    if let Some(device) = live.and_then(|l| l.get(DEVICE)) {
        out.insert(DEVICE.to_owned(), device.clone());
    }
    Ok(serde_json::to_vec(&Value::Object(out)).expect("a Value always serializes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compose_takes_the_device_key_from_live_absence_included() {
        let target = json!({"fa": {"token": "t"}, "device": {"id": "stale"}});
        let live = json!({"fa": {"token": "old"}, "device": {"id": "now"}});
        let out: Value = serde_json::from_slice(
            &compose(target.to_string().as_bytes(), live.as_object()).unwrap(),
        )
        .unwrap();
        assert_eq!(out, json!({"fa": {"token": "t"}, "device": {"id": "now"}}));
        let out: Value =
            serde_json::from_slice(&compose(target.to_string().as_bytes(), None).unwrap()).unwrap();
        assert_eq!(out, json!({"fa": {"token": "t"}}));
    }

    #[test]
    fn an_identity_needs_a_handle() {
        assert!(identity_from(&json!({"workspace": "ws"})).is_none());
        assert!(identity_from(&json!({"handle": ""})).is_none());
        assert_eq!(identity_from(&json!({"handle": "h"})).unwrap().label, "h");
    }
}
