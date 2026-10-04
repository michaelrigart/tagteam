//! §12.5 "Profile provenance" and "Lazy capture": a quiescent profile's credential against the
//! vault's, through the profile's seed, never by expiry (B.52). It runs under the account's
//! lock, in the refresh gate (§7.3 step 3) and in `switch`'s transaction (§9.2).

use std::path::Path;

use tagteam_core::{ProvenanceVerdict, provenance};
use tagteam_provider::profile::{ProfileMarker, Seed};
use tagteam_provider::{Identity, Provenance, Provider, Read, ReadError};

use crate::account_lock::AccountLock;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::session::SessionState;
use crate::store::AccountRow;

/// What applying a profile's provenance found, and did (§12.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileCheck {
    /// No profile, not quiescent, no seed (never bootstrapped), or its identity drifted.
    NotApplicable,
    InStep,
    Captured,
    VaultMovedOn,
    ReplacementWins,
    Conflict,
    /// The seed, the marker, the profile credential or the profile identity could not be read,
    /// or the marker is missing beside a seed, or names another account or provider, or the
    /// profile rotated but names no identity (Decision 9).
    Unreadable(String),
}

/// §12.5 "Identity drift": the profile's login is another account's than `row`. The email is
/// compared, or the label for a provider whose identities carry none, and the organization
/// only when both sides name one. The one definition: the session-owned usage fetch
/// (`collect.rs`, Task 12) calls it too.
pub(crate) fn identity_drifted(identity: &Identity, row: &AccountRow) -> bool {
    let email = identity.email.as_deref().unwrap_or(&identity.label)
        != row.email.as_deref().unwrap_or(&row.label);
    let org = !identity.org_uuid.is_empty()
        && !row.org_uuid.is_empty()
        && identity.org_uuid != row.org_uuid;
    email || org
}

/// A profile file's read error, as `ProfileCheck::Unreadable` carries it.
fn unreadable(e: &ReadError) -> ProfileCheck {
    ProfileCheck::Unreadable(format!("{}: {}", e.what, e.detail))
}

impl Engine {
    /// §12.5 under `lock` (the account's): reads the seed, the marker's spelling and the
    /// profile credential; applies `tagteam_core::provenance`; captures through
    /// `persist_generation` and moves the seed on `Capture`; reseeds on `InStep { reseed: true }`.
    pub(crate) fn apply_provenance(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
    ) -> Result<ProfileCheck, EngineError> {
        self.apply_provenance_leaving_out(p, row, lock, None)
    }

    /// `apply_provenance` for a profile quiescent apart from `own`, this process's launch
    /// reservation (§12.5 "When the child exits": the last session out captures).
    pub(crate) fn apply_provenance_apart_from(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        own: &Path,
    ) -> Result<ProfileCheck, EngineError> {
        self.apply_provenance_leaving_out(p, row, lock, Some(own))
    }

    /// The body of both.
    ///
    /// A capture needs a quiescent profile with a seed, the same identity, a fresh read of a
    /// credential with a refresh token (§6.2), and the table's `Capture` row, which a stale
    /// mark never reaches. Expiry is never consulted (B.52). A seed that cannot be moved after a
    /// capture or a reseed is an error: going on would make the next comparison a false
    /// `Conflict`. An unreadable vault is `EngineError::Unreadable`, which the caller reports as
    /// it reports its own vault reads. Unreadable also covers a marker that is absent beside a
    /// seed or names another account, a profile identity that cannot be read, and an absent
    /// one where the table says `Capture` (Decision 9: neither ignored nor captured). An absent
    /// identity decides nothing in any other row.
    fn apply_provenance_leaving_out(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        lock: &AccountLock,
        own: Option<&Path>,
    ) -> Result<ProfileCheck, EngineError> {
        debug_assert_eq!(lock.id(), &row.id, "the caller holds this account's lock");
        let state = match own {
            Some(own) => self.session_state_apart_from(p, row, own)?,
            None => self.session_state(p, row)?,
        };
        let SessionState::Quiescent { profile } = state else {
            return Ok(ProfileCheck::NotApplicable);
        };
        let seed = match Seed::read(&profile) {
            Read::Present(seed) => seed,
            Read::Absent => return Ok(ProfileCheck::NotApplicable),
            Read::Unreadable(e) => return Ok(unreadable(&e)),
        };
        let marker = match ProfileMarker::read(&profile) {
            Read::Present(m) if m.account_id == row.id && m.provider == row.provider => m,
            Read::Present(_) => {
                return Ok(ProfileCheck::Unreadable(format!(
                    "the marker in {} names another account",
                    profile.display()
                )));
            }
            Read::Absent => {
                return Ok(ProfileCheck::Unreadable(format!(
                    "{} has a seed but no marker",
                    profile.display()
                )));
            }
            Read::Unreadable(e) => return Ok(unreadable(&e)),
        };
        // §12.2: the profile's Keychain item is named from the recorded spelling, never one
        // derived again; its files are read in `profile`, where the profile is now.
        let spelling = marker.config_dir.as_str();
        let identity_absent = match p.profile_identity(&self.env, &profile) {
            Read::Present(login) if !identity_drifted(&login, row) => false,
            // Another account's login: the profile is ignored (§12.5).
            Read::Present(_) => return Ok(ProfileCheck::NotApplicable),
            // None to compare. Only a capture needs one (below).
            Read::Absent => true,
            Read::Unreadable(e) => return Ok(unreadable(&e)),
        };
        let held = match p.read_profile_credential(&self.env, &profile, spelling) {
            Read::Present(c) if c.provenance() == Provenance::Degraded => {
                return Ok(ProfileCheck::Unreadable(format!(
                    "the credential of {} could be read only from its file, which may be out of date",
                    profile.display()
                )));
            }
            Read::Present(c) => c.bytes().to_vec(),
            Read::Absent => return Ok(ProfileCheck::NotApplicable),
            Read::Unreadable(e) => return Ok(unreadable(&e)),
        };
        // §6.2: only a credential with a refresh token is a generation the vault may take.
        let Some(p_fp) = p.fingerprint(&held).filter(|_| p.has_refresh_token(&held)) else {
            return Ok(ProfileCheck::NotApplicable);
        };
        let v_fp = match self.vault.read(&row.id) {
            Read::Present(bytes) => match p.fingerprint(&bytes) {
                Some(fp) => fp,
                None => return Ok(ProfileCheck::NotApplicable),
            },
            Read::Absent => return Ok(ProfileCheck::NotApplicable),
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let stale = seed.login_epoch != row.login_epoch;
        let verdict = provenance(p_fp.as_str(), v_fp.as_str(), &seed.seed_fp, stale);
        // Decision 9: by its provenance the profile rotated the vault's generation, but no
        // identity says the rotation is the account's. Capturing it would be a guess, and
        // ignoring it would let the gate send the generation it consumed. No other row needs
        // the identity: in step, the vault moved on and a replacement wins, the profile holds
        // no rotation of the vault's generation, and a conflict stops everything anyway.
        if identity_absent && verdict == ProvenanceVerdict::Capture {
            return Ok(ProfileCheck::Unreadable(format!(
                "{} holds a rotated login but names no identity, so it cannot be told to be the account's",
                profile.display()
            )));
        }
        Ok(match verdict {
            ProvenanceVerdict::InStep { reseed } => {
                if reseed {
                    Seed {
                        seed_fp: v_fp.as_str().to_owned(),
                        ..seed
                    }
                    .write(&profile)?;
                }
                ProfileCheck::InStep
            }
            ProvenanceVerdict::Capture => {
                self.persist_generation(p, row, lock, &held)?;
                Seed {
                    seed_fp: p_fp.as_str().to_owned(),
                    ..seed
                }
                .write(&profile)?;
                tracing::info!(
                    position = row.position,
                    account = %row.id,
                    "captured the session profile's rotated login into the vault"
                );
                ProfileCheck::Captured
            }
            ProvenanceVerdict::VaultMovedOn => ProfileCheck::VaultMovedOn,
            ProvenanceVerdict::ReplacementWins => ProfileCheck::ReplacementWins,
            ProvenanceVerdict::Conflict => {
                tracing::warn!(
                    position = row.position,
                    account = %row.id,
                    "the session profile and the vault both moved since they last agreed; nothing is captured or refreshed until `tagteam add` replaces the login"
                );
                ProfileCheck::Conflict
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tagteam_core::{AccountId, ProviderId};

    fn row(email: Option<&str>, label: &str, org: &str) -> AccountRow {
        AccountRow {
            id: AccountId::from_string("0192"),
            provider: ProviderId::new("claude-code"),
            position: 1,
            identity_key: String::new(),
            label: label.into(),
            email: email.map(str::to_owned),
            org_uuid: org.into(),
            org_name: None,
            account_uuid: None,
            kind: "oauth".into(),
            alias: None,
            disabled: false,
            identity_json: serde_json::json!({}),
            login_expires_at: None,
            login_epoch: 0,
            replacing_fp: None,
            quarantine_reason: None,
            quarantine_fp: None,
            quarantine_at: None,
            added_at: 1,
        }
    }

    fn identity(email: Option<&str>, label: &str, org: &str) -> Identity {
        Identity {
            label: label.into(),
            email: email.map(str::to_owned),
            org_uuid: org.into(),
            org_name: None,
            account_uuid: None,
            raw: serde_json::json!({}),
        }
    }

    #[test]
    fn drift_compares_the_email_and_the_organization_only_when_both_name_one() {
        let ours = row(Some("a@x.co"), "a@x.co", "org-1");
        assert!(!identity_drifted(
            &identity(Some("a@x.co"), "a@x.co", "org-1"),
            &ours
        ));
        assert!(
            !identity_drifted(&identity(Some("a@x.co"), "a@x.co", ""), &ours),
            "only one side names an organization"
        );
        assert!(
            identity_drifted(&identity(Some("b@x.co"), "b@x.co", "org-1"), &ours),
            "another email"
        );
        assert!(
            identity_drifted(&identity(Some("a@x.co"), "a@x.co", "org-2"), &ours),
            "another organization"
        );
        let handle = row(None, "alice@ws", "ws");
        assert!(
            !identity_drifted(&identity(None, "alice@ws", "ws"), &handle),
            "no email: the label is compared"
        );
        assert!(identity_drifted(&identity(None, "bob@ws", "ws"), &handle));
    }
}
