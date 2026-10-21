//! Review refactor §8.4: the walk both never-delete-the-live-login guards share. `lifecycle.rs`
//! decides what one profile or orphan entry would break, and `purge.rs` what a whole purge would
//! delete; each keeps its own idea of what is doomed and spared, and its own refusal's wording and
//! log line. What they share is how a live file is resolved and which filesystem identities it
//! touches.

use std::fs;
use std::path::{Path, PathBuf};

use tagteam_provider::{Env, Provider};

use crate::error::{DeletionRefusal, EngineError, GuardScope};
use crate::fsutil::{FileId, absent, trace_path};

/// Why the walk refused, in the scope that asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveGuardError {
    /// A path the live login lives at could not be resolved.
    Unresolved(GuardScope),
    /// The live login's files are inside what would be deleted.
    Inside(GuardScope),
}

impl From<LiveGuardError> for EngineError {
    fn from(e: LiveGuardError) -> Self {
        let why = match e {
            LiveGuardError::Unresolved(scope) => DeletionRefusal::LiveLoginUnresolved(scope),
            LiveGuardError::Inside(scope) => DeletionRefusal::LiveLoginInside(scope),
        };
        EngineError::DeletionRefused { why }
    }
}

/// `p`'s identity surface: its credential files, then its JSON files.
pub(crate) fn live_files(p: &dyn Provider, env: &Env) -> Vec<PathBuf> {
    let surface = p.identity_surface(env);
    surface
        .credential_files
        .into_iter()
        .chain(surface.json_keys.into_iter().map(|(file, _)| file))
        .collect()
}

/// Walks each of `files` as the kernel resolves it (`trace_path`), and judges every path that
/// touches on its own chain (`touched_chain`) as soon as it is walked: `Inside(scope)` when
/// `inside(&chain)`. A path that cannot be traced or whose ancestors cannot be read is
/// `Unresolved(scope)`. One chain per path, not per file: a live file that links from a deleted
/// directory into a spared one is judged where it passes through the deleted one.
pub(crate) fn refuse_inside(
    files: &[PathBuf],
    scope: GuardScope,
    inside: impl Fn(&[FileId]) -> bool,
) -> Result<(), LiveGuardError> {
    for file in files {
        let touched = trace_path(file).map_err(|_| LiveGuardError::Unresolved(scope))?;
        for path in &touched {
            if inside(&touched_chain(path, scope)?) {
                return Err(LiveGuardError::Inside(scope));
            }
        }
    }
    Ok(())
}

/// `path`, then each ancestor: its `symlink_metadata` id, then its `metadata` id when that
/// resolves (a link to a directory counts as the directory). An entry that is not there ends the
/// chain, since nothing beyond it can be lost; any other `symlink_metadata` failure is
/// `Unresolved(scope)`.
fn touched_chain(path: &Path, scope: GuardScope) -> Result<Vec<FileId>, LiveGuardError> {
    let mut chain = Vec::new();
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => chain.push(FileId::of(&meta)),
            Err(e) if absent(&e) => break,
            Err(_) => return Err(LiveGuardError::Unresolved(scope)),
        }
        if let Ok(meta) = fs::metadata(ancestor) {
            chain.push(FileId::of(&meta));
        }
    }
    Ok(chain)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::error::GuardScope::{ProfileEntry, Purge};

    /// A canonical temp root.
    fn root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        (dir, root)
    }

    /// `p`'s identity, unfollowed.
    fn id(p: &Path) -> FileId {
        FileId::of(&fs::symlink_metadata(p).unwrap())
    }

    /// `p`'s identity, followed.
    fn fid(p: &Path) -> FileId {
        FileId::of(&fs::metadata(p).unwrap())
    }

    #[test]
    fn a_chain_holds_each_ancestor_unfollowed_and_followed() {
        let (_dir, t) = root();
        fs::create_dir_all(t.join("real")).unwrap();
        fs::write(t.join("real/x"), "x").unwrap();
        symlink(t.join("real"), t.join("link")).unwrap();
        let chain = touched_chain(&t.join("link/x"), Purge).unwrap();
        assert_eq!(chain[0], id(&t.join("real/x")));
        for want in [id(&t.join("link")), fid(&t.join("link")), id(&t)] {
            assert!(chain.contains(&want), "{chain:?}");
        }
    }

    #[test]
    fn an_absent_entry_ends_the_chain() {
        let (_dir, t) = root();
        assert_eq!(touched_chain(&t.join("none/x"), Purge).unwrap(), vec![]);
    }

    #[test]
    fn an_unreadable_ancestor_is_unresolved_in_its_scope() {
        let (_dir, t) = root();
        fs::create_dir_all(t.join("shut")).unwrap();
        fs::set_permissions(t.join("shut"), fs::Permissions::from_mode(0o000)).unwrap();
        let chain = touched_chain(&t.join("shut/x"), ProfileEntry);
        fs::set_permissions(t.join("shut"), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(chain, Err(LiveGuardError::Unresolved(ProfileEntry))),
            "{chain:?}"
        );
    }

    #[test]
    fn each_touched_path_is_judged_on_its_own_chain() {
        let (_dir, t) = root();
        fs::create_dir_all(t.join("wall")).unwrap();
        fs::create_dir_all(t.join("spared")).unwrap();
        fs::write(t.join("spared/f"), "x").unwrap();
        symlink(t.join("spared/f"), t.join("wall/live")).unwrap();
        let wall = fid(&t.join("wall"));
        let spared = fid(&t.join("spared"));
        let inside = |c: &[FileId]| c.contains(&wall) && !c.contains(&spared);
        let refused = refuse_inside(&[t.join("wall/live")], Purge, inside);
        assert!(
            matches!(refused, Err(LiveGuardError::Inside(Purge))),
            "{refused:?}"
        );
        assert!(refuse_inside(&[t.join("spared/f")], Purge, inside).is_ok());
    }
}
