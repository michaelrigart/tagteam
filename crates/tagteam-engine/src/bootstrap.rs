//! §12.3 steps 2–8: a quiescent profile's bootstrap from the vault, and its validation. The
//! launch (§12.5, `launch.rs`) calls it under `MutationGuard` and the account lock, which stay
//! held through validation; step 1's gate refresh ran before it took them.

use std::fs;
use std::path::Path;

#[cfg(feature = "test-hooks")]
use tagteam_core::AccountId;
use tagteam_core::Fingerprint;
use tagteam_provider::atomic::ensure_private_dir;
#[cfg(feature = "test-hooks")]
use tagteam_provider::profile::profile_path;
use tagteam_provider::profile::{ProfileMarker, Seed, canonical_profile_path};
use tagteam_provider::provider::Validity;
use tagteam_provider::{MutationGuard, Provenance, Provider, Read, ReadError};

use crate::account_lock::AccountLock;
use crate::displace::displace;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hooks;
use crate::provenance::identity_drifted;
use crate::store::AccountRow;

/// Why a quiescent profile needs a bootstrap (§12.3's opening list), in the order
/// `bootstrap_trigger` checks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// No seed: never bootstrapped.
    Missing,
    /// The per-launch check found the login `invalid` (`Seed.needs_bootstrap`).
    Invalid,
    /// The seed's epoch is not the account's `login_epoch` (§12.5).
    StaleMarked,
    /// The canonical path is no longer the spelling the marker records (§12.2).
    SpellingChanged,
    /// No credential, another generation than the vault's, or another login (§12.5).
    OtherCredential,
}

impl Trigger {
    /// For the log.
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Missing => "missing",
            Trigger::Invalid => "invalid",
            Trigger::StaleMarked => "stale-marked",
            Trigger::SpellingChanged => "spelling-changed",
            Trigger::OtherCredential => "other-credential",
        }
    }
}

/// §12.2 "One spelling": the profile's canonical path as the provider exports it. A path that
/// is not UTF-8 is refused: its lossy text would name another directory and Keychain item.
fn current_spelling(p: &dyn Provider, profile: &Path) -> Result<String, EngineError> {
    let canonical = canonical_profile_path(profile)?;
    if canonical.to_str().is_none() {
        return Err(EngineError::InvalidInput(format!(
            "{} is not valid UTF-8, so it cannot be exported as a session's config dir",
            canonical.display()
        )));
    }
    Ok(p.profile_spelling(&canonical))
}

/// The refusal for a profile path that is there but is not a real directory, as M4a's
/// `is_real_dir` decides it: a link, even to a directory, or a file. A profile is resolved from
/// its canonical spelling, so through a link at its path the marker, the links, the seed and
/// the session's credential would all land at the link's target (`~/.claude`, say).
fn not_its_own_directory(profile: &Path) -> EngineError {
    EngineError::InvalidInput(format!(
        "{} is not a directory of its own (a link, say), so a session's files and credential would be written wherever it leads; replace it with the directory itself, then run again",
        profile.display()
    ))
}

/// Refuses a profile path that is there but is not a real directory (`not_its_own_directory`),
/// before anything is read or written through it. A missing one is fine: it is created 0700.
pub(crate) fn refuse_linked_profile(profile: &Path) -> Result<(), EngineError> {
    if fs::symlink_metadata(profile).is_ok_and(|m| !m.is_dir()) {
        return Err(not_its_own_directory(profile));
    }
    Ok(())
}

/// §12.3's validation table, but for `invalid`'s deletion: `None` launches.
pub(crate) fn refusal(row: &AccountRow, validity: Validity) -> Option<EngineError> {
    let position = row.position;
    match validity {
        Validity::Valid => None,
        Validity::Invalid(detail) => Some(EngineError::LoginInvalid { position, detail }),
        Validity::Overridden { method, source } => Some(EngineError::LoginOverridden {
            position,
            method,
            key_source: source,
        }),
        Validity::Drifted { reported } => Some(EngineError::LoginDrifted { position, reported }),
        Validity::Unknown(detail) => Some(EngineError::LoginUnknown { position, detail }),
        Validity::Unreachable(detail) => Some(EngineError::LaunchUnreachable { detail }),
    }
}

impl Engine {
    /// The profile's marker, when it names `row`. A marker naming another account or provider
    /// refuses: its spelling names someone else's Keychain item, which is never touched here.
    pub(crate) fn own_marker(
        &self,
        row: &AccountRow,
        profile: &Path,
    ) -> Result<Option<ProfileMarker>, EngineError> {
        match ProfileMarker::read(profile) {
            Read::Present(m) if m.account_id == row.id && m.provider == row.provider => Ok(Some(m)),
            Read::Present(_) => Err(EngineError::InvalidInput(format!(
                "the marker in {} names another account; move the profile aside, then run again",
                profile.display()
            ))),
            Read::Absent => Ok(None),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }

    /// §12.2 "Profile marker", for a quiescent launch: a first launch creates the profile
    /// (0700) and its marker, whose `configDir` is the current spelling; a later one updates
    /// `outer` to this launch's home, before the links are synced from it. Only a bootstrap
    /// changes `configDir` (§12.3 step 6). A profile path that is there but is not a real
    /// directory refuses first, before anything is written through it.
    pub(crate) fn mark_profile(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
    ) -> Result<ProfileMarker, EngineError> {
        refuse_linked_profile(profile)?;
        let outer = p.outer_home(&self.env);
        let marker = match self.own_marker(row, profile)? {
            Some(m) if m.outer == outer => return Ok(m),
            Some(m) => ProfileMarker { outer, ..m },
            None => {
                ensure_private_dir(profile)?;
                ProfileMarker {
                    provider: row.provider.clone(),
                    account_id: row.id.clone(),
                    config_dir: current_spelling(p, profile)?,
                    outer,
                }
            }
        };
        marker.write(profile)?;
        Ok(marker)
    }

    /// The vault's current generation for `row`, which a bootstrap writes into the profile.
    pub(crate) fn vault_generation(&self, row: &AccountRow) -> Result<Vec<u8>, EngineError> {
        match self.vault.read(&row.id) {
            Read::Present(b) if !b.is_empty() => Ok(b),
            Read::Unreadable(source) => Err(EngineError::UnreadableAccount {
                position: row.position,
                label: row.label.clone(),
                source,
            }),
            _ => Err(EngineError::InvalidInput(format!(
                "{} (position {}) has no stored credential; log in and run `tagteam add` again",
                row.label, row.position
            ))),
        }
    }

    /// §12.3 step 2: the credential `profile` holds, read as Claude Code reads it: the item named
    /// from `spelling`, then the file in `profile`, its actual directory (Decision 22); `None`
    /// when it has none. Unreadable, degraded (the file covered an item that could not be read,
    /// so it may be an older generation) and empty (a Keychain timeout can look empty) are
    /// errors: tagteam never overwrites a credential it could not read.
    fn read_held(
        &self,
        p: &dyn Provider,
        profile: &Path,
        spelling: &str,
    ) -> Result<Option<Vec<u8>>, EngineError> {
        let damaged = |detail: &str| {
            EngineError::Unreadable(ReadError::new(
                format!("the credential of {}", profile.display()),
                detail,
            ))
        };
        match p.read_profile_credential(&self.env, profile, spelling) {
            Read::Present(c) if c.provenance() == Provenance::Degraded => Err(damaged(
                "its Keychain item could not be read, and the file that covered it may be out of date",
            )),
            Read::Present(c) if c.is_empty() => Err(damaged(
                "it read back empty, which a Keychain timeout can cause",
            )),
            Read::Present(c) => Ok(Some(c.bytes().to_vec())),
            Read::Absent => Ok(None),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }

    /// §12.3: which trigger applies to the quiescent `profile` now, if any, in `Trigger`'s
    /// order. `mark_profile` has run, so the marker exists. Reads only.
    pub(crate) fn bootstrap_trigger(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
    ) -> Result<Option<Trigger>, EngineError> {
        let seed = match Seed::read(profile) {
            Read::Present(s) => s,
            Read::Absent => return Ok(Some(Trigger::Missing)),
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        if seed.needs_bootstrap {
            return Ok(Some(Trigger::Invalid));
        }
        if seed.login_epoch != row.login_epoch {
            return Ok(Some(Trigger::StaleMarked));
        }
        let Some(marker) = self.own_marker(row, profile)? else {
            return Ok(Some(Trigger::Missing));
        };
        if marker.config_dir != current_spelling(p, profile)? {
            return Ok(Some(Trigger::SpellingChanged));
        }
        let vault_fp = p.fingerprint(&self.vault_generation(row)?);
        let Some(held) = self.read_held(p, profile, &marker.config_dir)? else {
            return Ok(Some(Trigger::OtherCredential));
        };
        if p.fingerprint(&held) != vault_fp {
            return Ok(Some(Trigger::OtherCredential));
        }
        match p.profile_identity(&self.env, profile) {
            Read::Present(login) if !identity_drifted(&login, row) => Ok(None),
            Read::Present(_) | Read::Absent => Ok(Some(Trigger::OtherCredential)),
            Read::Unreadable(e) => Err(EngineError::Unreadable(e)),
        }
    }

    /// §12.4: a baseline waiting in `profile`, left by a session whose merge-back never ran (its
    /// `tagteam` was killed), is merged back into the outer home's config first. A failure
    /// aborts with the profile and its baseline as they were: a seed over them would drop the
    /// session's changes, and its baseline write could follow a link at the baseline's path.
    /// The merge-back takes the outer home's config lock alone (§4.3), and only after
    /// `MutationGuard` and the account lock. The launch (Task 10) runs it before the marker and
    /// reports its summary; this keeps every seed and bootstrap behind the same rule.
    fn merge_back_waiting(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
    ) -> Result<(), EngineError> {
        if !p.has_baseline(profile) {
            return Ok(());
        }
        let report = p.merge_back(&self.env, profile, self.cancel())?;
        if !report.conflicts.is_empty() {
            tracing::warn!(
                position = row.position,
                account = %row.id,
                conflicts = ?report.conflicts,
                "a session's merge-back kept the default file's values where both sides changed"
            );
        }
        Ok(())
    }

    /// §12.4's seed of `profile`, the profile's actual directory, whatever spelling its marker
    /// records (Decision 22), after a waiting baseline is merged back.
    pub(crate) fn seed_of(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
    ) -> Result<(), EngineError> {
        self.merge_back_waiting(p, row, profile)?;
        p.seed_profile(&self.env, profile, &p.parse_identity(&row.identity_json)?)?;
        Ok(())
    }

    /// §12.3 steps 2–8 for the quiescent `profile`, whose marker exists. The caller holds
    /// `guard` and `lock` (this account's) throughout, validation included. `cwd` is the
    /// directory `claude` will run in, and `program` the launch command `plan_run` resolved,
    /// which the validation spawns (Decision 20). A profile that is not a real directory
    /// refuses, and a waiting baseline is merged back (§12.4), before anything is written.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn bootstrap_profile(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
        cwd: &Path,
        program: &Path,
        guard: &MutationGuard,
        lock: &AccountLock,
    ) -> Result<(), EngineError> {
        debug_assert_eq!(lock.id(), &row.id, "the caller holds this account's lock");
        // Only into a profile that is a real directory, as M4a's `is_real_dir` decides it: the
        // step 4 write resolves the profile from its canonical spelling, so through a link at
        // the profile path it would take the locks, and write the vault's account keys, at the
        // link's target. (The provider refuses a link at the credential file itself.)
        if !fs::symlink_metadata(profile).is_ok_and(|m| m.is_dir()) {
            return Err(not_its_own_directory(profile));
        }
        // §12.4: before anything touches the profile, so a failure leaves it as it was.
        self.merge_back_waiting(p, row, profile)?;
        // §6.2 "Pending rescues before activation": a rescue consumed the vault's generation.
        self.settle_rescues(p, row, lock)?;
        let vault = self.vault_generation(row)?;
        let v_fp = p.fingerprint(&vault).ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "position {}'s stored credential has no generation to start a session from",
                row.position
            ))
        })?;
        let marker = self.own_marker(row, profile)?.ok_or_else(|| {
            EngineError::InvalidInput(format!("{} has no profile marker", profile.display()))
        })?;
        let recorded = marker.config_dir.clone();
        let current = current_spelling(p, profile)?;
        // 2. As the profile holds it: the recorded spelling's item, then the file where the
        //    profile is now (Decision 22).
        let held = self.read_held(p, profile, &recorded)?;
        // 3.
        if let Some(bytes) = &held {
            self.displace_held(p, row, profile, bytes, &v_fp)?;
        }
        // 4. Account-scoped keys from the vault, machine-shared ones from the profile's own. The
        //    write takes the profile's credential locks and storage-write lock, under the
        //    current spelling, and releases them before it returns: step 5 takes them again.
        let composed = p.compose_profile_credential(&vault, held.as_deref())?;
        p.write_profile_credential(&self.env, &current, guard, &composed)?;
        hooks::point(self, "bootstrap-after-credential-write")?;
        // 5. Always, whatever the reason: an item left behind stays authoritative over the file.
        p.delete_profile_credential(&self.env, profile, &current)?;
        if recorded != current {
            p.delete_profile_credential(&self.env, profile, &recorded)?;
        }
        // 6. What Claude Code will read now must be the vault's current generation.
        let effective = self.read_held(p, profile, &current)?;
        let in_step = effective.as_deref().is_some_and(|e| {
            p.fingerprint(e).as_ref() == Some(&v_fp)
                && p.access_fingerprint(e) == p.access_fingerprint(&vault)
        });
        if !in_step {
            return Err(EngineError::InvalidInput(format!(
                "the credential a session of position {} would read is not the vault's current generation, so it was not started",
                row.position
            )));
        }
        Seed {
            login_epoch: row.login_epoch,
            seed_fp: v_fp.as_str().to_owned(),
            needs_bootstrap: false,
        }
        .write(profile)?;
        if recorded != current {
            ProfileMarker {
                config_dir: current.clone(),
                ..marker
            }
            .write(profile)?;
        }
        // 7.
        self.seed_of(p, row, profile)?;
        // 8.
        self.validate_bootstrap(p, row, &current, cwd, program)
    }

    /// §12.3 step 3, and B.5: a stale-marked profile's credential may be a live generation of
    /// the login a replacement superseded, and one whose identity drifted is another login
    /// altogether. Either is saved to `displaced/` before step 4 overwrites it; a failed save
    /// aborts. The vault's own generation, or an older one of this account, is not saved. The
    /// identity is read in `profile`, where the profile is now (Decision 22). Once the
    /// credential is not the vault's, a seed or identity that cannot be read aborts (§4.3):
    /// whose login it is could not be told, so it is never overwritten without a copy.
    fn displace_held(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        profile: &Path,
        held: &[u8],
        v_fp: &Fingerprint,
    ) -> Result<(), EngineError> {
        let held_fp = p.fingerprint(held);
        if held_fp.as_ref() == Some(v_fp) {
            return Ok(());
        }
        let stale = match Seed::read(profile) {
            Read::Present(s) => s.login_epoch != row.login_epoch,
            Read::Absent => false,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let identity = match p.profile_identity(&self.env, profile) {
            Read::Present(i) => Some(i),
            Read::Absent => None,
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let foreign = identity.as_ref().is_some_and(|i| identity_drifted(i, row));
        let reason = match (stale, foreign) {
            (_, true) => "foreign-profile-login",
            (true, false) => "replaced-profile-login",
            (false, false) => return Ok(()),
        };
        let raw = identity.map(|i| i.raw);
        let id = displace(
            self,
            &row.provider,
            held,
            held_fp.as_ref(),
            reason,
            raw.as_ref(),
        )?;
        tracing::warn!(
            position = row.position,
            account = %row.id,
            displaced = %id,
            reason,
            "saved the session profile's credential before bootstrapping over it"
        );
        Ok(())
    }

    /// §12.3 step 8 at a bootstrap, with both locks held. A signal recorded by the end of the
    /// check (whose process the spawner killed) interrupts the launch. `invalid` deletes the
    /// profile (B.29), unless it holds history that deleting it would lose or split (§12.2,
    /// R9.1): that refuses as the split and keeps it. Every other refusal keeps it.
    fn validate_bootstrap(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        spelling: &str,
        cwd: &Path,
        program: &Path,
    ) -> Result<(), EngineError> {
        let identity = p.parse_identity(&row.identity_json)?;
        let validity = p.validate_profile(
            &self.env,
            spelling,
            cwd,
            program,
            &identity,
            self.spawner.as_ref(),
            self.cancel(),
        );
        // §12.5 "Signals": read through the token, never through the `Validity` text.
        if let Some(signal) = self.cancel().requested() {
            return Err(EngineError::Interrupted(signal));
        }
        if matches!(validity, Validity::Invalid(_)) {
            // As every other deleter of a profile (`refuse_destroying`): real history is never
            // deleted or split silently.
            self.refuse_profile_split(p, row)?;
            tracing::warn!(
                position = row.position,
                account = %row.id,
                "the session profile's login is invalid; deleting the profile"
            );
            self.remove_profile(p, row)?;
        }
        refusal(row, validity).map_or(Ok(()), Err)
    }
}

#[cfg(feature = "test-hooks")]
impl Engine {
    /// Tests only (Task 9, before `launch` exists): what a quiescent launch runs between its
    /// locks and its reservation, provenance aside. That is the marker, the link sync, then a
    /// bootstrap for a §12.3 trigger, else the seed, under `MutationGuard` (30 s) and the
    /// account lock. Returns the trigger that applied. With no plan to resolve it, the
    /// validation's program is the launch command by name: the scripted spawner records it and
    /// never runs it.
    pub fn bootstrap_quiescent(
        &self,
        id: &AccountId,
        cwd: &Path,
    ) -> Result<Option<Trigger>, EngineError> {
        let guard = MutationGuard::acquire(&self.env, MutationGuard::BOOTSTRAP_TIMEOUT)?;
        let lock = self.lock_account(id)?;
        let row = self
            .store()?
            .account(id)?
            .ok_or_else(|| EngineError::NoSuchAccount(id.to_string()))?;
        let p = self.provider(&row.provider)?;
        let p = p.as_ref();
        let profile = profile_path(&self.env, &row.id);
        self.mark_profile(p, &row, &profile)?;
        self.sync_profile_links(p, &profile, false)?;
        let trigger = self.bootstrap_trigger(p, &row, &profile)?;
        let program = Path::new(p.launch_command());
        match trigger {
            Some(_) => self.bootstrap_profile(p, &row, &profile, cwd, program, &guard, &lock)?,
            None => self.seed_of(p, &row, &profile)?,
        }
        Ok(trigger)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tagteam_provider::profile::{MARKER_FILE, profile_path};

    use super::*;
    use crate::testutil::{NOW, T, cred};

    #[test]
    fn a_bootstrap_of_a_linked_profile_refuses_before_anything_is_written() {
        // Behind `mark_profile`, which a launch runs first and which refuses the same path: the
        // bootstrap itself never writes through a link at the profile path (controller ruling).
        // The target holds the account's own marker, as a profile moved and linked back does,
        // so nothing but the link stops the bootstrap.
        let t = T::new();
        let row = t.account("a@x.co", &cred("rt-a", NOW + 86_400_000));
        let p = t.engine.provider(&row.provider).unwrap();
        let profile = profile_path(&t.env, &row.id);
        let target = t.env.home.join("elsewhere");
        fs::create_dir_all(&target).unwrap();
        ProfileMarker {
            provider: row.provider.clone(),
            account_id: row.id.clone(),
            config_dir: current_spelling(p.as_ref(), &target).unwrap(),
            outer: p.outer_home(&t.env),
        }
        .write(&target)
        .unwrap();
        fs::create_dir_all(profile.parent().unwrap()).unwrap();
        symlink(&target, &profile).unwrap();
        let guard = MutationGuard::acquire(&t.env, MutationGuard::TIMEOUT).unwrap();
        let lock = t.lock(&row.id);

        let err = t
            .engine
            .bootstrap_profile(
                p.as_ref(),
                &row,
                &profile,
                &t.env.home,
                Path::new("claude"),
                &guard,
                &lock,
            )
            .unwrap_err();

        assert_eq!(err.kind(), "invalid-input", "{err}");
        assert!(
            err.to_string().contains(&profile.display().to_string()),
            "{err}"
        );
        let held: Vec<_> = fs::read_dir(&target)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(held, [MARKER_FILE], "nothing written at the target");
    }
}
