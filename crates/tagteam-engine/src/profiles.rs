//! §12.2's link sync. A profile shares the outer home's entries by allowlist: the provider
//! names the source home, the shared and must-share entries and the known-private patterns
//! (`Provider::share_policy`, Decision 14), and `run.share_extra` adds names. Each link points
//! at the fully resolved source, because Claude Code follows at most one link when it writes
//! through one (Appendix A.1). tagteam removes only the links it made, as `.tagteam-links.json`
//! records them, and never replaces, merges or deletes a real file or directory.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, symlink};
use std::path::{Path, PathBuf};

use tagteam_provider::atomic::ensure_private_dir;
use tagteam_provider::profile::{LinksRecord, MARKER_FILE, ProfileMarker, entry_matches};
use tagteam_provider::{EntryKind, Provider, Read, SharePolicy};

use crate::engine::Engine;
use crate::error::{EngineError, SplitCause};
use crate::settings::is_share_name;

/// What one sync did, by entry name, and the lines it reports.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncReport {
    /// Links made in the profile.
    pub created: Vec<String>,
    /// tagteam's own links taken out of the profile. A link made again, to a source that
    /// moved, is in both lists.
    pub removed: Vec<String>,
    /// For stderr: a shared entry the profile holds as a real copy, a link tagteam did not
    /// make, a private name in `run.share_extra`, and an unknown entry of the source home,
    /// noted once (§12.2).
    pub warnings: Vec<String>,
}

/// tagteam's own files in a profile, never linked whatever a policy says (§12.2).
const OWN_PREFIX: &str = ".tagteam-";

/// One allowlisted entry, and the kind a must-share one is created as.
struct Wanted {
    name: String,
    must: Option<EntryKind>,
}

/// What the profile holds where a link may belong.
enum Held {
    Nothing,
    /// A symbolic link, and the target it was made with.
    Link(PathBuf),
    /// A real file or directory.
    Real,
}

impl Engine {
    /// §12.2's sync of the profile at `profile`, whose marker names `p`. The caller holds
    /// `MutationGuard` and the account lock (§12.5, launch step 3); the sync takes no lock of
    /// its own. The source home is the one the marker's `outer` records (§4.5 `outer_home`): a
    /// quiescent launch has just updated it, and a joining one leaves it as the running session
    /// found it.
    ///
    /// 1. Nothing is written until the source home and every must-share entry have been
    ///    checked. One the profile holds as a real copy, or as a link tagteam did not make that
    ///    resolves elsewhere, refuses with `ProfileSplit`, naming both paths and the cause; a
    ///    must-share source, or a source home, that is a link to nothing (or loops) refuses too,
    ///    and so does a source home that is not a directory.
    /// 2. Unless `joining`, each link tagteam made (as recorded, and still as made) is removed
    ///    when its entry left the allowlist, its source disappeared, or its source now resolves
    ///    elsewhere (it moved, or the outer home changed). A source still there is linked again.
    /// 3. A must-share entry missing from the source home is created there, empty (§3's
    ///    create-only row). Every allowlisted entry the source holds and the profile lacks is
    ///    linked to its fully resolved path. A real file or directory where a link belongs, or
    ///    a link tagteam did not make that resolves elsewhere, is left alone with a warning
    ///    naming both paths.
    /// 4. An entry of the source home on no list is noted once (§12.2 "Unknown entries").
    ///
    /// The record is written, atomically, whenever it changed, even after a step failed, so it
    /// always names exactly the links tagteam made.
    pub fn sync_profile_links(
        &self,
        p: &dyn Provider,
        profile: &Path,
        joining: bool,
    ) -> Result<SyncReport, EngineError> {
        if !p.capabilities().sessions {
            return Err(EngineError::InvalidInput(format!(
                "{} has no `tagteam run` sessions",
                p.display_name()
            )));
        }
        let marker = match ProfileMarker::read(profile) {
            Read::Present(m) => m,
            Read::Absent => {
                return Err(EngineError::InvalidInput(format!(
                    "{} is missing, so the profile's outer home is unknown",
                    profile.join(MARKER_FILE).display()
                )));
            }
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        if marker.provider != p.id() {
            return Err(EngineError::InvalidInput(format!(
                "{} is a {} profile, not a {} one",
                profile.display(),
                marker.provider,
                p.id()
            )));
        }
        let policy = p.share_policy(&p.apply_outer_home(&self.env, &marker.outer)?);
        let before = match LinksRecord::read(profile) {
            Read::Present(r) => r,
            Read::Absent => LinksRecord::default(),
            Read::Unreadable(e) => return Err(EngineError::Unreadable(e)),
        };
        let mut report = SyncReport::default();
        let wanted = allowlist(&policy, &self.settings.share_extra, &mut report.warnings);
        let unknown = check(&policy, profile, &wanted, &before, joining)?;
        let mut record = before.clone();
        let applied = apply(&policy, profile, &wanted, joining, &mut record, &mut report);
        for name in unknown {
            if record.noted_unknown.insert(name.clone()) {
                report.warnings.push(format!(
                    "{} is on none of {}'s share lists, so it stays private to each profile",
                    policy.source.join(&name).display(),
                    p.display_name()
                ));
            }
        }
        let written = if record == before {
            Ok(())
        } else {
            record.write(profile)
        };
        applied?;
        written?;
        Ok(report)
    }
}

/// The must-share entries, then the shared ones, then `run.share_extra`, without repeats. A
/// private name, or one of tagteam's own, is never on it; one named in `run.share_extra`
/// warns.
fn allowlist(policy: &SharePolicy, extra: &[String], warnings: &mut Vec<String>) -> Vec<Wanted> {
    fn add(wanted: &mut Vec<Wanted>, name: &str, must: Option<EntryKind>) {
        if !wanted.iter().any(|w| w.name == name) {
            wanted.push(Wanted {
                name: name.to_owned(),
                must,
            });
        }
    }
    let mut wanted = Vec::new();
    for m in &policy.must_share {
        if !is_private(policy, m.name) {
            add(&mut wanted, m.name, Some(m.kind));
        }
    }
    for name in &policy.shared {
        if !is_private(policy, name) {
            add(&mut wanted, name, None);
        }
    }
    for name in extra {
        if !is_share_name(name) {
            // `Settings` drops these already; a hand-built one is held to the same rule.
            warnings.push(format!(
                "run.share_extra: {name:?} is not an entry name, so it is not shared"
            ));
        } else if is_private(policy, name) {
            warnings.push(format!(
                "run.share_extra: {name} is private to each profile, so it is not shared"
            ));
        } else {
            add(&mut wanted, name, None);
        }
    }
    wanted
}

/// Known-private (`entry_matches` against the policy's patterns), or tagteam's own.
fn is_private(policy: &SharePolicy, name: &str) -> bool {
    name.starts_with(OWN_PREFIX)
        || policy
            .private
            .iter()
            .any(|pattern| entry_matches(pattern, name))
}

/// Step 1, which writes nothing: every refusal, and the source home's entries on no list.
/// `joining`: a session runs, so a must-share link of tagteam's that no longer resolves where
/// the source does cannot be made again under it, and is a split (§12.2: memory and history
/// are never split).
fn check(
    policy: &SharePolicy,
    profile: &Path,
    wanted: &[Wanted],
    record: &LinksRecord,
    joining: bool,
) -> Result<Vec<String>, EngineError> {
    match resolved(&policy.source)? {
        // Nothing could be created in it, and nothing shared from it.
        None if exists(&policy.source)? => return Err(link_to_nothing(&policy.source)),
        Some(home) if !home.is_dir() => {
            return Err(EngineError::InvalidInput(format!(
                "{} is not a directory, so nothing in it can be shared; fix or remove it, then run again",
                policy.source.display()
            )));
        }
        Some(home) if fs::canonicalize(profile).is_ok_and(|own| own == home) => {
            return Err(EngineError::InvalidInput(format!(
                "the outer home of {} is the profile itself, so it has nothing to share",
                profile.display()
            )));
        }
        _ => {}
    }
    for w in wanted.iter().filter(|w| w.must.is_some()) {
        let (src, dst) = (policy.source.join(&w.name), profile.join(&w.name));
        let target = resolved(&src)?;
        if target.is_none() && exists(&src)? {
            return Err(link_to_nothing(&src));
        }
        let cause = match held(&dst)? {
            Held::Nothing => None,
            Held::Real => Some(SplitCause::RealCopy),
            // tagteam's own link, which step 2 keeps or makes again; a join cannot make it
            // again, so one that no longer points where the source resolves is a split. A
            // missing source is created empty by step 2 at the source home's own path, so the
            // link stays good when it points exactly there.
            Held::Link(made) if record.links.get(&w.name) == Some(&made) => {
                let stale = match &target {
                    Some(t) => t != &made,
                    None => {
                        resolved(&policy.source)?
                            .map(|home| home.join(&w.name))
                            .as_ref()
                            != Some(&made)
                    }
                };
                (joining && stale).then_some(SplitCause::StaleWhileRunning)
            }
            // Anyone else's is the share only if it resolves where the source does.
            Held::Link(_) => {
                (target.is_none() || resolved(&dst)? != target).then_some(SplitCause::LinkElsewhere)
            }
        };
        if let Some(cause) = cause {
            return Err(EngineError::ProfileSplit {
                profile: dst,
                shared: src,
                cause,
            });
        }
    }
    let mut unknown = Vec::new();
    let listing = match fs::read_dir(&policy.source) {
        Ok(listing) => listing,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(unknown),
        Err(e) => return Err(e.into()),
    };
    for entry in listing {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if !is_private(policy, &name) && !wanted.iter().any(|w| w.name == name) {
            unknown.push(name);
        }
    }
    unknown.sort();
    Ok(unknown)
}

/// Steps 2 and 3. Each change is recorded in `record` the moment it is made, so a failure part
/// way leaves a record naming exactly the links tagteam made.
fn apply(
    policy: &SharePolicy,
    profile: &Path,
    wanted: &[Wanted],
    joining: bool,
    record: &mut LinksRecord,
    report: &mut SyncReport,
) -> Result<(), EngineError> {
    if !joining {
        let left: Vec<String> = record
            .links
            .keys()
            .filter(|name| !wanted.iter().any(|w| w.name == **name))
            .cloned()
            .collect();
        for name in left {
            unlink_ours(profile, &name, record, report)?;
        }
    }
    for w in wanted {
        let (src, dst) = (policy.source.join(&w.name), profile.join(&w.name));
        if let Some(kind) = w.must {
            create_empty(&policy.source, &src, kind)?;
        }
        let target = resolved(&src)?;
        match held(&dst)? {
            Held::Nothing => {
                // A recorded link someone removed is no longer tagteam's.
                record.links.remove(&w.name);
                if let Some(target) = target {
                    link(w, &src, &dst, target, record, report)?;
                }
            }
            Held::Link(made) if record.links.get(&w.name) == Some(&made) => {
                if joining || target.as_ref() == Some(&made) {
                    continue;
                }
                fs::remove_file(&dst)?;
                record.links.remove(&w.name);
                report.removed.push(w.name.clone());
                if let Some(target) = target {
                    link(w, &src, &dst, target, record, report)?;
                }
            }
            Held::Link(made) => {
                record.links.remove(&w.name);
                if target.is_some() && resolved(&dst)? != target {
                    report.warnings.push(format!(
                        "{} links to {} rather than {}; tagteam did not make that link and leaves it as it is",
                        dst.display(),
                        made.display(),
                        src.display()
                    ));
                }
            }
            Held::Real => {
                record.links.remove(&w.name);
                if target.is_some() {
                    report.warnings.push(format!(
                        "{} is a real copy where {} should be linked; tagteam leaves both as they are",
                        dst.display(),
                        src.display()
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Removes `name`'s link when it is still the one tagteam made, and nothing else, and forgets
/// it either way.
fn unlink_ours(
    profile: &Path,
    name: &str,
    record: &mut LinksRecord,
    report: &mut SyncReport,
) -> Result<(), EngineError> {
    let dst = profile.join(name);
    let ours = match (record.links.get(name), held(&dst)?) {
        (Some(made), Held::Link(now)) => *made == now,
        _ => false,
    };
    if ours {
        fs::remove_file(&dst)?;
        report.removed.push(name.to_owned());
    }
    record.links.remove(name);
    Ok(())
}

/// Links `dst` to `target`, the fully resolved `src`, and records it. Something that appeared
/// at `dst` since it was checked is never replaced: for a must-share entry that is a split.
fn link(
    w: &Wanted,
    src: &Path,
    dst: &Path,
    target: PathBuf,
    record: &mut LinksRecord,
    report: &mut SyncReport,
) -> Result<(), EngineError> {
    match symlink(&target, dst) {
        Ok(()) => {
            record.links.insert(w.name.clone(), target);
            report.created.push(w.name.clone());
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists && w.must.is_some() => {
            let cause = match held(dst) {
                Ok(Held::Link(_)) => SplitCause::LinkElsewhere,
                _ => SplitCause::RealCopy,
            };
            Err(EngineError::ProfileSplit {
                profile: dst.to_path_buf(),
                shared: src.to_path_buf(),
                cause,
            })
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            report.warnings.push(format!(
                "{} appeared while it was being linked; tagteam leaves it as it is",
                dst.display()
            ));
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// §3's create-only row: a must-share entry the source home lacks is created there, empty, as
/// the agent itself would create it, and only when there is none (step 1 has refused a link to
/// nothing). A source home that does not exist yet is created first, 0700.
fn create_empty(source: &Path, src: &Path, kind: EntryKind) -> io::Result<()> {
    if exists(src)? {
        return Ok(());
    }
    ensure_private_dir(source)?;
    let made = match kind {
        EntryKind::Dir => DirBuilder::new().mode(0o700).create(src),
        EntryKind::File => OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(src)
            .map(drop),
    };
    match made {
        // The agent created it meanwhile: it exists, which is all this wants.
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(()),
        other => other,
    }
}

fn held(path: &Path) -> io::Result<Held> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Ok(Held::Link(fs::read_link(path)?)),
        Ok(_) => Ok(Held::Real),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Held::Nothing),
        Err(e) => Err(e),
    }
}

/// The refusal for `path`, a link that resolves to nothing: dangling, looping, or through a
/// file.
fn link_to_nothing(path: &Path) -> EngineError {
    EngineError::InvalidInput(format!(
        "{} is a link to nothing, so it cannot be shared; fix or remove it, then run again",
        path.display()
    ))
}

/// Whether `e` says a path names nothing: it is absent, or a link on the way to it dangles,
/// loops (`ELOOP`) or passes through a file (`ENOTDIR`).
fn names_nothing(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory)
        || e.raw_os_error() == Some(libc::ELOOP)
}

/// The fully resolved path of `path`, or `None` when there is nothing to resolve (absent, or a
/// link to nothing, `names_nothing`).
fn resolved(path: &Path) -> io::Result<Option<PathBuf>> {
    match fs::canonicalize(path) {
        Ok(p) => Ok(Some(p)),
        Err(e) if names_nothing(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether `path` is an entry of its own, a link to nothing included. Under something that is
/// not a directory there is none.
fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if names_nothing(&e) => Ok(false),
        Err(e) => Err(e),
    }
}
