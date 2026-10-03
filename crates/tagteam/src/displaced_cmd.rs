//! `tagteam displaced` (§6.3): the listing, for a person and as JSON. It never prints a
//! credential.

use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_core::ProviderId;
use tagteam_engine::Engine;
use tagteam_engine::displace::{DisplacedEntry, DisplacedList};

use crate::render::{self, MISSING};

const EMPTY: &str = "No displaced credentials.\n";
const HEADS: [&str; 7] = [
    "ID", "WHEN", "PROVIDER", "REASON", "IDENTITY", "ACCOUNT", "FP",
];
/// An entry that names no identity, or one this build cannot read.
const UNKNOWN: &str = "unknown";
/// A file with no row, and a row with no file (§6.3).
const UNRECORDED: &str = "unrecorded";
const FILE_MISSING: &str = "file missing";

/// When an entry was displaced, in UTC to the second. This is the JSON's `at`, on the log's
/// clock (§14.2), so an entry can be matched to the switch that made it.
fn when(e: &DisplacedEntry) -> String {
    format_iso8601(e.at_ms.div_euclid(1000))
}

/// A fingerprint's first 12 hex digits (§6.3), as file names carry them.
fn fp12(fingerprint: &str) -> &str {
    let hex = fingerprint.strip_prefix("sha256:").unwrap_or(fingerprint);
    hex.get(..12).unwrap_or(hex)
}

/// The identity `e` names, as `list` names an account: its email, with its organization in
/// brackets. It is read through the entry's provider, so the CLI names no provider's identity
/// fields. `None` when the entry names no identity, or none this build can read.
pub(crate) fn identity(engine: &Engine, e: &DisplacedEntry) -> Option<String> {
    let provider = engine.provider(e.provider.as_ref()?).ok()?;
    let identity = provider.parse_identity(e.identity.as_ref()?).ok()?;
    let email = identity.email.unwrap_or(identity.label);
    Some(match identity.org_name {
        Some(org) => format!("{email} [{org}]"),
        None => email,
    })
}

/// One entry's cells, in `HEADS`' order, and its state when it has no row or no file.
fn cells(
    e: &DisplacedEntry,
    identity: &dyn Fn(&DisplacedEntry) -> Option<String>,
) -> ([String; 7], Option<&'static str>) {
    let or_missing = |s: Option<String>| s.unwrap_or_else(|| MISSING.to_owned());
    let cells = [
        e.id.clone(),
        when(e),
        or_missing(e.provider.as_ref().map(|p| p.as_str().to_owned())),
        or_missing(e.reason.clone()),
        identity(e).unwrap_or_else(|| UNKNOWN.to_owned()),
        or_missing(e.account.map(|n| format!("#{n}"))),
        or_missing(e.fingerprint.as_deref().map(|f| fp12(f).to_owned())),
    ];
    let state = if !e.recorded {
        Some(UNRECORDED)
    } else if !e.file_present {
        Some(FILE_MISSING)
    } else {
        None
    };
    (cells, state)
}

/// §6.3's listing for a person: one row per entry, newest first, aligned by display width,
/// with a trailing state for an entry that has no row or no file. The directory the files
/// are in comes last. `identity` names the identity an entry carries.
pub(crate) fn human(
    list: &DisplacedList,
    identity: &dyn Fn(&DisplacedEntry) -> Option<String>,
) -> String {
    if list.entries.is_empty() {
        return EMPTY.into();
    }
    let rows: Vec<([String; 7], Option<&str>)> =
        list.entries.iter().map(|e| cells(e, identity)).collect();
    let widths: Vec<usize> = HEADS
        .iter()
        .enumerate()
        .map(|(i, head)| {
            rows.iter()
                .map(|(c, _)| render::width(&c[i]))
                .fold(render::width(head), usize::max)
        })
        .collect();
    let line = |cells: &[&str], state: Option<&str>| {
        let mut s = cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| render::pad(c, *w))
            .collect::<Vec<_>>()
            .join("  ");
        if let Some(state) = state {
            s.push_str("  ");
            s.push_str(state);
        }
        format!("{}\n", s.trim_end())
    };
    let mut out = line(&HEADS, None);
    for (c, state) in &rows {
        let c: Vec<&str> = c.iter().map(String::as_str).collect();
        out.push_str(&line(&c, *state));
    }
    out.push_str(&format!(
        "\nThe files are {}/<ID>.json; tagteam never reads one back, so restoring one is manual.\n",
        list.dir.display()
    ));
    out
}

/// §6.3's object: `{schemaVersion, dir, displaced: [{id, provider, at, reason, fingerprint,
/// identity, account, file, recorded}]}`. `at` is in ISO 8601 UTC, and whatever is not known
/// is null.
pub(crate) fn json(list: &DisplacedList) -> Value {
    let displaced: Vec<Value> = list
        .entries
        .iter()
        .map(|e| {
            json!({
                "id": e.id,
                "provider": e.provider.as_ref().map(ProviderId::as_str),
                "at": when(e),
                "reason": e.reason,
                "fingerprint": e.fingerprint,
                "identity": e.identity,
                "account": e.account,
                "file": if e.file_present { "present" } else { "missing" },
                "recorded": e.recorded,
            })
        })
        .collect();
    json!({"schemaVersion": 1, "dir": list.dir.to_string_lossy(), "displaced": displaced})
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;
    use tagteam_core::ProviderId;

    use super::*;

    const NEWEST: &str = "1790000300-0123456789ab-aaaaaa";
    const MIDDLE: &str = "1790000250-fedcba987654-bbbbbb";
    const OLDEST: &str = "1790000200-00112233aabb-cccccc";

    /// The fingerprint a recorded entry carries: its ID's 12 hex digits, then zeros.
    fn fingerprint(id: &str) -> String {
        format!("sha256:{}{}", &id[11..23], "0".repeat(52))
    }

    fn recorded(
        id: &str,
        at_ms: i64,
        email: &str,
        account: Option<u32>,
        file_present: bool,
    ) -> DisplacedEntry {
        DisplacedEntry {
            id: id.into(),
            provider: Some(ProviderId::new("claude-code")),
            at_ms,
            reason: Some("displaced-live-login".into()),
            fingerprint: Some(fingerprint(id)),
            identity: Some(json!({"emailAddress": email})),
            account,
            file_present,
            recorded: true,
        }
    }

    /// A row and its file naming b (#2), a file with no row, and a row whose file is gone.
    fn sample() -> DisplacedList {
        DisplacedList {
            dir: PathBuf::from("/h/.local/share/tagteam/displaced"),
            entries: vec![
                recorded(NEWEST, 1_790_000_300_500, "b@x.co", Some(2), true),
                DisplacedEntry {
                    id: MIDDLE.into(),
                    provider: None,
                    at_ms: 1_790_000_250_000,
                    reason: None,
                    fingerprint: None,
                    identity: None,
                    account: None,
                    file_present: true,
                    recorded: false,
                },
                recorded(OLDEST, 1_790_000_200_000, "stranger@x.co", None, false),
            ],
        }
    }

    /// The identity's email: what the app's provider-backed `identity` reads from a CC identity.
    fn email(e: &DisplacedEntry) -> Option<String> {
        e.identity.as_ref()?["emailAddress"]
            .as_str()
            .map(str::to_owned)
    }

    #[test]
    fn the_listing_is_a_table_newest_first_then_the_directory() {
        assert_eq!(
            human(&sample(), &email),
            concat!(
                "ID                              WHEN                  PROVIDER     REASON                IDENTITY       ACCOUNT  FP\n",
                "1790000300-0123456789ab-aaaaaa  2026-09-21T14:18:20Z  claude-code  displaced-live-login  b@x.co         #2       0123456789ab\n",
                "1790000250-fedcba987654-bbbbbb  2026-09-21T14:17:30Z  —            —                     unknown        —        —             unrecorded\n",
                "1790000200-00112233aabb-cccccc  2026-09-21T14:16:40Z  claude-code  displaced-live-login  stranger@x.co  —        00112233aabb  file missing\n",
                "\n",
                "The files are /h/.local/share/tagteam/displaced/<ID>.json; tagteam never reads one back, so restoring one is manual.\n",
            )
        );
    }

    #[test]
    fn the_json_is_section_6_3_s_object() {
        assert_eq!(
            json(&sample()),
            json!({
                "schemaVersion": 1,
                "dir": "/h/.local/share/tagteam/displaced",
                "displaced": [
                    {"id": NEWEST, "provider": "claude-code", "at": "2026-09-21T14:18:20Z",
                     "reason": "displaced-live-login", "fingerprint": fingerprint(NEWEST),
                     "identity": {"emailAddress": "b@x.co"}, "account": 2, "file": "present",
                     "recorded": true},
                    {"id": MIDDLE, "provider": null, "at": "2026-09-21T14:17:30Z", "reason": null,
                     "fingerprint": null, "identity": null, "account": null, "file": "present",
                     "recorded": false},
                    {"id": OLDEST, "provider": "claude-code", "at": "2026-09-21T14:16:40Z",
                     "reason": "displaced-live-login", "fingerprint": fingerprint(OLDEST),
                     "identity": {"emailAddress": "stranger@x.co"}, "account": null,
                     "file": "missing", "recorded": true},
                ]
            })
        );
        // In §6.3's field order, which `preserve_order` keeps.
        let text = json(&sample()).to_string();
        assert!(
            text.starts_with(&format!(
                r#"{{"schemaVersion":1,"dir":"/h/.local/share/tagteam/displaced","displaced":[{{"id":"{NEWEST}","provider":"claude-code","at":"#
            )),
            "{text}"
        );
    }

    #[test]
    fn an_empty_listing_says_so() {
        let empty = DisplacedList {
            dir: PathBuf::from("/d"),
            entries: Vec::new(),
        };
        assert_eq!(human(&empty, &email), "No displaced credentials.\n");
        assert_eq!(
            json(&empty),
            json!({"schemaVersion": 1, "dir": "/d", "displaced": []})
        );
    }
}
