use serde_json::Value;
use tagteam_core::{Fingerprint, ProviderId};
use tagteam_provider::atomic::{ensure_private_dir, write_atomic};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::store::DisplacedRow;

/// Stashes live credential bytes that are about to be overwritten (§6.3). Forensic and
/// write-only; always a plain 0600 file, never a Keychain item.
#[expect(dead_code, reason = "used by the switch, Task 18")]
pub(crate) fn displace(
    engine: &Engine,
    provider: &ProviderId,
    bytes: &[u8],
    fp: Option<&Fingerprint>,
    reason: &str,
    identity: Option<&Value>,
) -> Result<String, EngineError> {
    let dir = engine.env().data_dir().join("displaced");
    ensure_private_dir(&dir)?;
    let now = engine.now_ms();
    let fp12 = fp.map_or_else(|| "000000000000".to_owned(), |f| f.short12().to_owned());
    let rand6: String = (0..6)
        .map(|_| fastrand::alphanumeric().to_ascii_lowercase())
        .collect();
    let id = format!("{}-{fp12}-{rand6}", now / 1000);
    write_atomic(&dir.join(format!("{id}.json")), bytes, 0o600)?;
    engine.store()?.insert_displaced(&DisplacedRow {
        id: id.clone(),
        provider: provider.clone(),
        at: now,
        reason: reason.to_owned(),
        fingerprint: fp.map(|f| f.as_str().to_owned()).unwrap_or_default(),
        identity: identity.cloned(),
    })?;
    Ok(id)
}
