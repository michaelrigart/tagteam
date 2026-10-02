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
    ///
    /// The profile is `profile_path(env, id)` (§5). A held reservation, then a live record,
    /// makes the account `Owned`; failing that, anything that could not be read makes it
    /// `Unreadable`. Every I/O failure is a state, never an error. A provider without
    /// `sessions` has no profiles, and nothing on disk is touched for it.
    pub fn session_state(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<SessionState, EngineError> {
        if !p.capabilities().sessions {
            return Ok(SessionState::NoProfile);
        }
        let profile = profile_path(&self.env, &row.id);
        match fs::symlink_metadata(&profile) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(SessionState::NoProfile),
            Err(e) => {
                let detail = format!("{}: {e}", profile.display());
                return Ok(unreadable_state(&profile, detail));
            }
        }
        match launch_reservations(&profile) {
            Read::Present(found) => {
                if found.iter().any(|(_, probe)| *probe == LockProbe::Held) {
                    return Ok(SessionState::Owned { profile });
                }
            }
            Read::Absent => {}
            Read::Unreadable(e) => return Ok(unreadable_state(&profile, e.to_string())),
        }
        let mut damaged = None;
        match read_session_records(&p.session_records_dir(&profile)) {
            Read::Present(entries) => {
                for entry in entries {
                    match entry {
                        RecordEntry::Record(r) => {
                            if record_is_live(self.process.as_ref(), &r, p.launch_command()) {
                                return Ok(SessionState::Owned { profile });
                            }
                        }
                        RecordEntry::Unreadable { path, detail } => {
                            damaged.get_or_insert_with(|| format!("{}: {detail}", path.display()));
                        }
                    }
                }
            }
            Read::Absent => {}
            Read::Unreadable(e) => return Ok(unreadable_state(&profile, e.to_string())),
        }
        Ok(match damaged {
            Some(detail) => unreadable_state(&profile, detail),
            None => SessionState::Quiescent { profile },
        })
    }

    /// §10.3 Guard and §9.2's session-owned target: refuses while `row` is session-owned. The
    /// answer holds while the caller holds the mutation lock and `row`'s account lock: no
    /// reservation is created without both (§12.5).
    pub(crate) fn refuse_session_owned(
        &self,
        p: &dyn Provider,
        row: &AccountRow,
    ) -> Result<(), EngineError> {
        let state = self.session_state(p, row)?;
        if let SessionState::Unreadable { detail, .. } = &state {
            tracing::warn!(
                position = row.position,
                account = %row.id,
                "a session reservation or record could not be read ({detail}); the account counts as session-owned"
            );
        }
        if state.owned() {
            return Err(EngineError::SessionOwned {
                position: row.position,
                label: row.label.clone(),
            });
        }
        Ok(())
    }
}
