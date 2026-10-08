//! §12.8: where a process stands, inside a run shell or not. It is found once, before the
//! engine is built (Decision 6), so every engine read of `env` already sees the default home.
//! And §12.5's session state: whether a session owns an account.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tagteam_provider::flock::LockProbe;
use tagteam_provider::liveness::{RecordEntry, read_session_records, record_is_live};
use tagteam_provider::profile::{
    MARKER_FILE, ProfileMarker, RunShell, launch_reservations, profile_path,
};
use tagteam_provider::{Env, Provider, Read};

use crate::engine::Engine;
use crate::error::EngineError;
use crate::registry::ProviderRegistry;
use crate::store::AccountRow;

fn unreadable_marker(dir: &Path, env: &Env, detail: String) -> (RunShell, Env) {
    (
        RunShell::Unreadable {
            marker: dir.join(MARKER_FILE),
            detail,
        },
        env.clone(),
    )
}

/// §12.8: the first registered provider with sessions whose `session_dir(env)` holds a marker
/// decides. Returns the run shell and the effective `Env` (the marker provider's
/// `apply_outer_home`, or `env` unchanged when `Outside` or `Unreadable`). The provider whose
/// variable found the marker must be the provider the marker names; a mismatch is `Unreadable`.
///
/// Only the marker is consulted (B.57): not where the directory lies, and not `XDG_DATA_HOME`,
/// which a run shell may have changed. A marker that names another provider leaves this
/// provider's outer home unknown, and so does an `outer` record the provider cannot apply, so
/// both count as unreadable.
pub fn detect_run_shell(env: &Env, registry: &ProviderRegistry) -> (RunShell, Env) {
    for p in registry.all() {
        if !p.capabilities().sessions {
            continue;
        }
        let Some(dir) = p.session_dir(env) else {
            continue;
        };
        let marker = match ProfileMarker::read(&dir) {
            Read::Absent => continue,
            Read::Unreadable(e) => return unreadable_marker(&dir, env, e.detail),
            Read::Present(m) => m,
        };
        if marker.provider != p.id() {
            let detail = format!("it names the provider {}, not {}", marker.provider, p.id());
            return unreadable_marker(&dir, env, detail);
        }
        return match p.apply_outer_home(env, &marker.outer) {
            Ok(outer) => (
                RunShell::Inside {
                    profile: dir,
                    marker,
                },
                outer,
            ),
            Err(e) => unreadable_marker(&dir, env, e.to_string()),
        };
    }
    (RunShell::Outside, env.clone())
}

/// §12.5: whether a session owns an account, computed on each call and never cached
/// (Decision 8).
#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    NoProfile,
    Quiescent {
        profile: PathBuf,
    },
    Owned {
        profile: PathBuf,
    },
    /// A reservation or a record could not be read: counts as owned (§10.3, §12.6).
    Unreadable {
        profile: PathBuf,
        detail: String,
    },
}

impl SessionState {
    /// Session-owned (§12.5): a live reservation or record, or one that could not be read.
    pub fn owned(&self) -> bool {
        matches!(
            self,
            SessionState::Owned { .. } | SessionState::Unreadable { .. }
        )
    }

    /// The profile directory, when the account has one.
    pub fn profile(&self) -> Option<&Path> {
        match self {
            SessionState::NoProfile => None,
            SessionState::Quiescent { profile }
            | SessionState::Owned { profile }
            | SessionState::Unreadable { profile, .. } => Some(profile),
        }
    }
}

fn unreadable_state(profile: &Path, detail: String) -> SessionState {
    SessionState::Unreadable {
        profile: profile.to_path_buf(),
        detail,
    }
}

impl Engine {
    /// §12.5: reservations (any `Held`) and session records (`record_is_live`, any `Unreadable`).
    pub fn session_state(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<SessionState, EngineError> {
        if !p.capabilities().sessions {
            return Ok(SessionState::NoProfile);
        }
        Ok(self.session_state_leaving_out(p, &profile_path(&self.env, &row.id), None))
    }

    /// `session_state`, with `own` left out: this process's own launch reservation, as §12.5
    /// "When the child exits" asks whether the profile is quiescent apart from it. It is
    /// matched by file name inside `.tagteam-launch/`.
    pub(crate) fn session_state_apart_from(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
        own: &Path,
    ) -> Result<SessionState, EngineError> {
        if !p.capabilities().sessions {
            return Ok(SessionState::NoProfile);
        }
        Ok(self.session_state_leaving_out(p, &profile_path(&self.env, &row.id), Some(own)))
    }

    /// `session_state` for the profile at `profile`, whatever account it belongs to: §10.5
    /// step 6 asks it of a profile that no store account owns. `p` judges its session records.
    pub(crate) fn session_state_at(&self, p: &dyn Provider, profile: &Path) -> SessionState {
        self.session_state_leaving_out(p, profile, None)
    }

    /// The body of all three, for the profile at `profile` (for an account,
    /// `profile_path(env, id)`, §5). A held reservation other than `own`, then a live record,
    /// makes it `Owned`; failing that, anything that could not be read makes it `Unreadable`.
    /// Every I/O failure is a state, never an error. The callers that take an account check
    /// first that its provider has `sessions`, so nothing on disk is touched for one without.
    fn session_state_leaving_out(
        &self,
        p: &dyn Provider,
        profile: &Path,
        own: Option<&Path>,
    ) -> SessionState {
        let profile = profile.to_path_buf();
        match fs::symlink_metadata(&profile) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return SessionState::NoProfile,
            Err(e) => {
                let detail = format!("{}: {e}", profile.display());
                return unreadable_state(&profile, detail);
            }
        }
        let is_own = |path: &Path| own.is_some_and(|own| path.file_name() == own.file_name());
        match launch_reservations(&profile) {
            Read::Present(found) => {
                if found
                    .iter()
                    .any(|(path, probe)| *probe == LockProbe::Held && !is_own(path))
                {
                    return SessionState::Owned { profile };
                }
            }
            Read::Absent => {}
            Read::Unreadable(e) => return unreadable_state(&profile, e.to_string()),
        }
        let mut damaged = None;
        match read_session_records(&p.session_records_dir(&profile)) {
            Read::Present(entries) => {
                for entry in entries {
                    match entry {
                        RecordEntry::Record(r) => {
                            if record_is_live(self.process.as_ref(), &r, p.launch_command()) {
                                return SessionState::Owned { profile };
                            }
                        }
                        RecordEntry::Unreadable { path, detail } => {
                            damaged.get_or_insert_with(|| format!("{}: {detail}", path.display()));
                        }
                    }
                }
            }
            Read::Absent => {}
            Read::Unreadable(e) => return unreadable_state(&profile, e.to_string()),
        }
        match damaged {
            Some(detail) => unreadable_state(&profile, detail),
            None => SessionState::Quiescent { profile },
        }
    }

    /// §10.3 Guard and §9.2's session-owned target: refuses while `row` is session-owned. The
    /// answer holds while the caller holds the mutation lock and `row`'s account lock: no
    /// reservation is created without both (§12.5). A reservation or record that cannot be read
    /// counts as owned, and the refusal names it and why (§12.6).
    pub(crate) fn refuse_session_owned(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<(), EngineError> {
        let state = self.session_state(p, row)?;
        let unreadable = match &state {
            // §14.2, B.69: the log gives the state only, since the detail names the file and a
            // record's name is not tagteam's to choose. The refusal names it to the user.
            SessionState::Unreadable { detail, .. } => {
                tracing::warn!(
                    position = row.position,
                    account = %row.id,
                    state = "unreadable",
                    "a session reservation or record could not be read; the account counts as session-owned"
                );
                Some(detail.clone())
            }
            _ => None,
        };
        if state.owned() {
            return Err(EngineError::SessionOwned {
                position: row.position,
                label: row.label.clone(),
                unreadable,
            });
        }
        Ok(())
    }
}
