//! §12.8: where a process stands, inside a run shell or not. It is found once, before the
//! engine is built (Decision 6), so every engine read of `env` already sees the default home.
//! And §12.5's session state: whether a session owns an account.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tagteam_provider::flock::LockProbe;
use tagteam_provider::liveness::{
    RecordEntry, read_session_records, read_supervisor_lock, record_is_live,
};
use tagteam_provider::profile::{
    MARKER_FILE, ProfileMarker, RunShell, launch_reservations, profile_path,
};
use tagteam_provider::{Env, Provider, Read};

use crate::engine::Engine;
use crate::error::{EngineError, SessionOwner};
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
/// (Decision 8). One aggregate: it carries every live owner and every unreadable input found,
/// since judging stops at nothing (§12.6); the texts about it come from `SessionOwner`.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    NoProfile,
    Quiescent {
        profile: PathBuf,
    },
    /// At least one live owner (§12.6): `session`, a held reservation or a record that is not a
    /// daemon's; `daemon`, a Claude Code background daemon (a `bg`, `daemon` or `daemon-worker`
    /// record, or the supervisor's lock file). Both can hold at once. `damaged` is every input
    /// that could not be read beside them.
    Owned {
        profile: PathBuf,
        session: bool,
        daemon: bool,
        damaged: Vec<Damaged>,
    },
    /// Nothing live was found, but an input could not be read: counts as owned (§10.3, §12.6).
    Unreadable {
        profile: PathBuf,
        damaged: Vec<Damaged>,
    },
}

/// An input to the session state that could not be read (§12.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Damaged {
    pub kind: DamagedKind,
    /// The file or directory, as the reader named it.
    pub file: PathBuf,
    /// Why, never quoting its bytes.
    pub detail: String,
}

/// Which input a `Damaged` is, which decides its remedy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamagedKind {
    /// The profile directory itself.
    Profile,
    /// The launch reservations directory.
    Reservations,
    /// The session records directory.
    RecordsDir,
    /// One session record.
    Record,
    /// The supervisor's lock file.
    SupervisorLock,
}

/// The `kind`s of a session record written by a Claude Code background daemon or its workers
/// (Appendix A.7).
const DAEMON_KINDS: [&str; 3] = ["bg", "daemon", "daemon-worker"];

impl SessionState {
    /// Session-owned (§12.5): a live reservation, record or supervisor, or one that could not be
    /// read.
    pub fn owned(&self) -> bool {
        matches!(
            self,
            SessionState::Owned { .. } | SessionState::Unreadable { .. }
        )
    }

    /// Every input that could not be read.
    pub fn damaged(&self) -> &[Damaged] {
        match self {
            SessionState::Owned { damaged, .. } | SessionState::Unreadable { damaged, .. } => {
                damaged
            }
            _ => &[],
        }
    }

    /// The profile directory, when a background daemon owns it (§12.6).
    pub fn daemon_profile(&self) -> Option<&Path> {
        match self {
            SessionState::Owned {
                profile,
                daemon: true,
                ..
            } => Some(profile),
            _ => None,
        }
    }

    /// The profile directory, when the account has one.
    pub fn profile(&self) -> Option<&Path> {
        match self {
            SessionState::NoProfile => None,
            SessionState::Quiescent { profile }
            | SessionState::Owned { profile, .. }
            | SessionState::Unreadable { profile, .. } => Some(profile),
        }
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
    /// `profile_path(env, id)`, §5). It judges every input and stops at none: the profile, the
    /// launch reservations other than `own`, the records directory, every record, and the
    /// supervisor's lock (§12.6). Any live owner makes it `Owned`, naming each; failing that,
    /// anything that could not be read makes it `Unreadable`; either carries every unreadable
    /// input found, with its file. Every I/O failure is a state, never an error. The callers
    /// that take an account check first that its provider has `sessions`, so nothing on disk is
    /// touched for one without.
    fn session_state_leaving_out(
        &self,
        p: &dyn Provider,
        profile: &Path,
        own: Option<&Path>,
    ) -> SessionState {
        let profile = profile.to_path_buf();
        let mut damaged = Vec::new();
        match fs::symlink_metadata(&profile) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return SessionState::NoProfile,
            Err(e) => damaged.push(Damaged {
                kind: DamagedKind::Profile,
                file: profile.clone(),
                detail: e.to_string(),
            }),
        }
        let is_own = |path: &Path| own.is_some_and(|own| path.file_name() == own.file_name());
        let (mut session, mut daemon) = (false, false);
        match launch_reservations(&profile) {
            Read::Present(found) => {
                session |= found
                    .iter()
                    .any(|(path, probe)| *probe == LockProbe::Held && !is_own(path));
            }
            Read::Absent => {}
            Read::Unreadable(e) => damaged.push(Damaged {
                kind: DamagedKind::Reservations,
                file: PathBuf::from(e.what),
                detail: e.detail,
            }),
        }
        match read_session_records(&p.session_records_dir(&profile)) {
            Read::Present(entries) => {
                for entry in entries {
                    match entry {
                        RecordEntry::Record(r) => {
                            if record_is_live(self.process.as_ref(), &r, p.launch_command()) {
                                if r.kind.as_deref().is_some_and(|k| DAEMON_KINDS.contains(&k)) {
                                    daemon = true;
                                } else {
                                    session = true;
                                }
                            }
                        }
                        RecordEntry::Unreadable { path, detail } => damaged.push(Damaged {
                            kind: DamagedKind::Record,
                            file: path,
                            detail,
                        }),
                    }
                }
            }
            Read::Absent => {}
            Read::Unreadable(e) => damaged.push(Damaged {
                kind: DamagedKind::RecordsDir,
                file: PathBuf::from(e.what),
                detail: e.detail,
            }),
        }
        // The supervisor of a background daemon writes no record of its own (CC 2.1.292); its
        // lock file is judged as a record is (§12.6).
        if let Some(lock) = p.supervisor_lock(&profile) {
            match read_supervisor_lock(&lock) {
                Read::Present(r) => {
                    daemon |= record_is_live(self.process.as_ref(), &r, p.launch_command());
                }
                Read::Absent => {}
                Read::Unreadable(e) => damaged.push(Damaged {
                    kind: DamagedKind::SupervisorLock,
                    file: lock,
                    detail: e.detail,
                }),
            }
        }
        if session || daemon {
            SessionState::Owned {
                profile,
                session,
                daemon,
                damaged,
            }
        } else if damaged.is_empty() {
            SessionState::Quiescent { profile }
        } else {
            SessionState::Unreadable { profile, damaged }
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
        // §14.2, B.69: the log gives the state only, since the detail names the file and a
        // record's name is not tagteam's to choose. The refusal names it to the user.
        if !state.damaged().is_empty() {
            tracing::warn!(
                position = row.position,
                account = %row.id,
                state = "unreadable",
                "a session reservation or record could not be read; the account counts as session-owned"
            );
        }
        if state.owned() {
            return Err(session_owned_error(row, &state));
        }
        Ok(())
    }
}

/// The `session-owned` refusal for `row`, whose profile is in `state`: it names every owner
/// found and, for state that could not be read, the file and its repair.
pub(crate) fn session_owned_error(row: &AccountRow, state: &SessionState) -> EngineError {
    EngineError::SessionOwned {
        position: row.position,
        label: row.label.clone(),
        owner: Box::new(SessionOwner::of(state)),
    }
}
