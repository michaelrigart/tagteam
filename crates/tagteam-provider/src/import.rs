use std::fmt;

use serde_json::{Map, Value};
use tagteam_core::ProviderId;

/// A file that is not a format import reads, or one malformed. The message names the account
/// and the field, never a value, which may be a secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct FormatError(pub String);

/// One account an import file holds: read and type-checked, not yet validated by its provider
/// (§13.3 pass 1). An account ID is never read from a file (§6.1, B.65). It holds a
/// credential, so `Debug` shows neither payload.
#[derive(Clone)]
pub struct ImportRecord {
    pub provider: ProviderId,
    pub position: u32,
    /// The file's kind, held against the provider's classification of the credential. `None`
    /// for a cswap account whose kind cswap leaves to the credential's shape.
    pub kind: Option<String>,
    /// For information: the stored label comes from the identity the provider validates.
    pub label: Option<String>,
    pub alias: Option<String>,
    pub disabled: bool,
    /// Epoch ms.
    pub added_at: Option<i64>,
    pub identity: Value,
    pub credential: Value,
}

impl fmt::Debug for ImportRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportRecord")
            .field("provider", &self.provider)
            .field("position", &self.position)
            .field("kind", &self.kind)
            .field("alias", &self.alias)
            .field("disabled", &self.disabled)
            .field("added_at", &self.added_at)
            .finish_non_exhaustive()
    }
}

/// An export format of another tool, which a provider reads as its own accounts.
pub struct ForeignFormat {
    /// What the format is called; `Decoded.foreign` carries it.
    pub name: &'static str,
    /// Whether a parsed file is in this format.
    pub detect: fn(&Value) -> bool,
    /// The file's accounts, as records for this provider.
    pub read: fn(&Value) -> Result<Vec<ImportRecord>, FormatError>,
}

/// What one account of a file failed, naming the field and the type it must have.
pub fn field_error(account: usize, field: &str, must_be: &str) -> FormatError {
    FormatError(format!("account {account}: {field} must be {must_be}"))
}

/// A string, or nothing when absent or null; any other type is refused.
pub fn optional_string(
    o: &Map<String, Value>,
    account: usize,
    field: &str,
) -> Result<Option<String>, FormatError> {
    match o.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(field_error(account, field, "a string or null")),
    }
}

/// An integer position ≥ 1 (§13.3 pass 1: a path-traversal defence, B.34).
pub fn position(o: &Map<String, Value>, account: usize, field: &str) -> Result<u32, FormatError> {
    o.get(field)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n >= 1)
        .ok_or_else(|| field_error(account, field, "an integer of at least 1"))
}

pub fn disabled(o: &Map<String, Value>, account: usize) -> Result<bool, FormatError> {
    match o.get("disabled") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(field_error(account, "disabled", "true or false")),
    }
}

/// A file's accounts, each numbered from 1 as the errors name them.
pub type Numbered = Vec<(usize, Map<String, Value>)>;

/// The `accounts` array, each entry an object.
pub fn accounts(v: &Value, what: &str) -> Result<Numbered, FormatError> {
    let list = v
        .get("accounts")
        .and_then(Value::as_array)
        .ok_or_else(|| FormatError(format!("{what} has no accounts list")))?;
    list.iter()
        .enumerate()
        .map(|(i, a)| match a {
            Value::Object(o) => Ok((i + 1, o.clone())),
            _ => Err(FormatError(format!(
                "account {}: it is not a JSON object",
                i + 1
            ))),
        })
        .collect()
}
