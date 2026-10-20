use tagteam_core::ProviderId;
use tagteam_provider::{DoomedEntry, Identity, LiveAuth, Provenance, Provider, Read, ReadError};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::session::{SessionState, session_owned_error};
use crate::store::{AccountRow, Store};

/// §9.4 step 3: never back up an empty value, because a Keychain timeout can look empty. It is
/// reported as unreadable, on either axis: the value could not be read with confidence.
fn empty_live_read(what: &str) -> EngineError {
    EngineError::Unreadable(ReadError::new(
        what,
        "it read back empty, which a Keychain timeout can cause; the switch was not attempted",
    ))
}

/// The two auth axes a live login can be on (§9.4 step 7): the credential entry, or the
/// managed API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Axis {
    Entry,
    ManagedKey,
}

impl Axis {
    pub(crate) const BOTH: [Axis; 2] = [Axis::Entry, Axis::ManagedKey];

    /// The axis `p` keeps a credential of `kind` on (§9.4 step 7).
    pub(crate) fn of(p: &dyn Provider, kind: &str) -> Self {
        if p.kind_traits(kind).managed_key_axis {
            Axis::ManagedKey
        } else {
            Axis::Entry
        }
    }

    pub(crate) fn other(self) -> Self {
        match self {
            Axis::Entry => Axis::ManagedKey,
            Axis::ManagedKey => Axis::Entry,
        }
    }

    /// The live secret on this axis. A degraded read is not a secret to act on (§4.3).
    pub(crate) fn live_secret(self, auth: &LiveAuth) -> Option<Vec<u8>> {
        match self {
            Axis::ManagedKey => auth.managed_key.as_ref().present().cloned(),
            Axis::Entry => auth
                .credential
                .as_ref()
                .present()
                .filter(|c| c.provenance() == Provenance::Fresh)
                .map(|c| c.bytes().to_vec()),
        }
    }
}

/// §9.4 step 3, with or without --force: what could not be read fresh is never overwritten,
/// on either axis. An empty value is never backed up, because a Keychain timeout can look
/// empty; a degraded entry may hide a newer generation.
pub(crate) fn refuse_unsafe_live_reads(live: &LiveAuth) -> Result<(), EngineError> {
    match &live.credential {
        Read::Unreadable(e) => return Err(EngineError::Unreadable(e.clone())),
        Read::Present(c) if c.provenance() == Provenance::Degraded => {
            return Err(EngineError::DegradedRead);
        }
        Read::Present(c) if c.is_empty() => return Err(empty_live_read("the live credential")),
        _ => {}
    }
    match &live.managed_key {
        Read::Unreadable(e) => Err(EngineError::Unreadable(e.clone())),
        Read::Present(k) if k.is_empty() => Err(empty_live_read("the live API key")),
        _ => Ok(()),
    }
}

/// Two secrets of one generation: equal bytes, or the same fingerprint.
pub(super) fn same_generation(p: &dyn Provider, a: &[u8], b: &[u8]) -> bool {
    let fp = p.fingerprint(a);
    a == b || (fp.is_some() && fp == p.fingerprint(b))
}

/// Generations already kept somewhere, by fingerprint (§9.4 step 7): destroying a live entry
/// of one loses nothing. An entry with no fingerprint holds nothing account-scoped to lose.
#[derive(Default)]
pub(crate) struct Held(Vec<String>);

impl Held {
    /// Records `fp`; false when it was already held.
    pub(crate) fn insert(&mut self, fp: &str) -> bool {
        if self.0.iter().any(|h| h == fp) {
            return false;
        }
        self.0.push(fp.to_owned());
        true
    }

    /// Whether `fp` is held already, without recording it.
    pub(crate) fn contains(&self, fp: &str) -> bool {
        self.0.iter().any(|h| h == fp)
    }

    /// Records the generation of `secret`, if it has one.
    pub(crate) fn hold(&mut self, p: &dyn Provider, secret: &[u8]) {
        if let Some(fp) = p.fingerprint(secret) {
            self.insert(fp.as_str());
        }
    }
}

/// §9.4 step 3, for every entry a change destroys: one that could not be read is never
/// overwritten or deleted, with or without --force.
pub(crate) fn refuse_unreadable(doomed: &[DoomedEntry]) -> Result<(), EngineError> {
    match doomed.iter().find_map(|d| match &d.bytes {
        Read::Unreadable(e) => Some(e),
        _ => None,
    }) {
        Some(e) => Err(EngineError::Unreadable(e.clone())),
        None => Ok(()),
    }
}

/// B.5: a failed displacement aborts the switch, except under --force, where it is a warning.
pub(super) fn unless_forced(
    saved: Result<(), EngineError>,
    force: bool,
    warnings: &mut Vec<String>,
) -> Result<(), EngineError> {
    match saved {
        Err(e) if force => {
            warnings.push(format!("could not save the previous live credential: {e}"));
            Ok(())
        }
        other => other,
    }
}

/// §6.3, Decision 13: the live identity names only the live login's own secret,
/// `own_secret`. A secret on the other auth axis is never attributed to the login beside it,
/// so its row names no identity.
pub(super) fn attributed<'i>(
    live_identity: Option<&'i Identity>,
    own_secret: Option<&[u8]>,
    bytes: &[u8],
) -> Option<&'i Identity> {
    live_identity.filter(|_| own_secret == Some(bytes))
}

/// The pre-lock oracle answer (§9.4 "Before locking"). It is about exact live bytes, not
/// about an account: it counts only while the live secret is still those bytes (§9.4 step 4),
/// and is attributed to an account only through `verdict` (§7.6).
pub(crate) struct OracleHint {
    pub(crate) bytes: Vec<u8>,
    pub(crate) resolved: Option<Identity>,
}

/// The oracle's answer about `bytes`, if it was asked about exactly these bytes.
pub(crate) fn answer_for<'h>(hint: Option<&'h OracleHint>, bytes: &[u8]) -> Option<&'h Identity> {
    hint.filter(|h| h.bytes == bytes)
        .and_then(|h| h.resolved.as_ref())
}

pub(crate) fn needs_relogin(p: &dyn Provider, target: &AccountRow) -> EngineError {
    EngineError::NeedsRelogin {
        position: target.position,
        label: target.label.clone(),
        launch: p.launch_command(),
    }
}

pub(crate) fn cannot_refresh(app: &str, label: &str, why: &str) -> String {
    format!("could not refresh {label} first ({why}); {app} will refresh it when it is online")
}

pub(crate) fn works_until_expiry(target: &AccountRow) -> String {
    format!(
        "{} (position {}) needs a new login: its stored refresh token can no longer be used; it works only until its current access token expires",
        target.label, target.position
    )
}

/// §9.2's session-owned refusal, for a target a session took after planning: it names what
/// owns the profile in `state`, as every other refusal does (§12.6).
pub(super) fn session_owned(
    p: &dyn Provider,
    target: &AccountRow,
    state: &SessionState,
) -> EngineError {
    session_owned_error(p, target, state)
}

/// §9.2's refusal for a target whose profile and vault both moved (§12.5).
pub(super) fn profile_conflict(target: &AccountRow) -> EngineError {
    EngineError::ProfileConflict {
        position: target.position,
        label: target.label.clone(),
    }
}

impl Engine {
    /// Whether `row`'s vault holds a credential. A vault that cannot be read is reported,
    /// naming the account, never taken for a missing credential (§4.3, §9.3).
    pub(super) fn vault_holds_login(&self, row: &AccountRow) -> Result<bool, EngineError> {
        match self.vault.read(&row.id) {
            Read::Present(b) => Ok(!b.is_empty()),
            Read::Absent => Ok(false),
            Read::Unreadable(source) => Err(EngineError::UnreadableAccount {
                position: row.position,
                label: row.label.clone(),
                source,
            }),
        }
    }

    /// An identity and a non-empty vault credential.
    pub(super) fn has_login(&self, row: &AccountRow) -> Result<bool, EngineError> {
        Ok(row.identity_json.is_object() && self.vault_holds_login(row)?)
    }

    pub(super) fn matches_vault(&self, p: &dyn Provider, row: &AccountRow, live: &[u8]) -> bool {
        match self.vault.read(&row.id) {
            Read::Present(v) => same_generation(p, &v, live),
            _ => false,
        }
    }

    /// The live identity, `None` when there is no live login; an unreadable one is an error,
    /// never taken for an absent one.
    pub(crate) fn read_live_identity(
        &self,
        p: &dyn Provider,
    ) -> Result<Option<Identity>, EngineError> {
        match p.live_identity(&self.env) {
            Read::Present(i) => Ok(Some(i)),
            Read::Absent => Ok(None),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }

    /// The live identity and the stored account it names, if any.
    pub(super) fn live_row(
        &self,
        p: &dyn Provider,
        store: &Store,
        provider: &ProviderId,
    ) -> Result<(Option<Identity>, Option<AccountRow>), EngineError> {
        let live = self.read_live_identity(p)?;
        let row = match &live {
            Some(i) => store.find_by_identity_key(provider, p.identity_key(i).as_str())?,
            None => None,
        };
        Ok((live, row))
    }
}
