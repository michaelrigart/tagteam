use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};

/// The most links `trace_path` follows, as the kernel's own limit.
const MAX_LINK_HOPS: u32 = 40;

/// Walks the absolute `path` component by component as the kernel resolves it, and returns
/// every location it touched: each link entry's own location (not what it leads to alone), each
/// directory on the way, and the final target. A link's target is spliced in at its place,
/// relative to the link's directory; `.` and `..` apply to the resolved prefix, which holds no
/// link. A component that is not there ends the walk with what was touched before it: nothing
/// beyond can be lost. More than `MAX_LINK_HOPS` links, a relative `path` and any other failure
/// are errors.
pub(crate) fn trace_path(path: &Path) -> io::Result<Vec<PathBuf>> {
    if !path.is_absolute() {
        return Err(io::Error::other("not an absolute path"));
    }
    let mut touched = Vec::new();
    let mut resolved = PathBuf::from("/");
    let mut queue: VecDeque<OsString> = names(path).collect();
    let mut hops = 0;
    while let Some(name) = queue.pop_front() {
        if name == "." {
            continue;
        }
        if name == ".." {
            resolved.pop();
            continue;
        }
        let next = resolved.join(&name);
        match fs::symlink_metadata(&next) {
            Ok(meta) if meta.file_type().is_symlink() => {
                hops += 1;
                if hops > MAX_LINK_HOPS {
                    return Err(io::Error::other("too many levels of symbolic links"));
                }
                touched.push(next.clone());
                let target = fs::read_link(&next)?;
                if target.is_absolute() {
                    resolved = PathBuf::from("/");
                }
                for part in names(&target).collect::<Vec<_>>().into_iter().rev() {
                    queue.push_front(part);
                }
            }
            Ok(_) => {
                touched.push(next.clone());
                resolved = next;
            }
            Err(e) if absent(&e) => return Ok(touched),
            Err(e) => return Err(e),
        }
    }
    Ok(touched)
}

/// The names `path` is made of, `.` and `..` included, without its root.
fn names(path: &Path) -> impl Iterator<Item = OsString> + '_ {
    path.components().filter_map(|c| match c {
        Component::Normal(n) => Some(n.to_os_string()),
        Component::CurDir => Some(".".into()),
        Component::ParentDir => Some("..".into()),
        Component::RootDir | Component::Prefix(_) => None,
    })
}

/// `(device, inode)`: what a file is, whatever it is called.
pub(crate) fn identity(meta: &fs::Metadata) -> (u64, u64) {
    (meta.dev(), meta.ino())
}

/// Whether `e` says the path is not there (or has a non-directory in the way).
pub(crate) fn absent(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    /// A canonical temp root, with `a/b/` and a file `a/b/f` in it.
    fn root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/b/f"), "x").unwrap();
        (dir, root)
    }

    #[test]
    fn a_plain_path_touches_each_directory_and_the_file() {
        let (_dir, r) = root();
        let touched = trace_path(&r.join("a/b/f")).unwrap();
        assert_eq!(
            &touched[touched.len() - 3..],
            [r.join("a"), r.join("a/b"), r.join("a/b/f")]
        );
    }

    #[test]
    fn a_final_link_touches_its_own_location_and_its_target() {
        let (_dir, r) = root();
        symlink(r.join("a/b/f"), r.join("link")).unwrap();
        let touched = trace_path(&r.join("link")).unwrap();
        assert!(touched.contains(&r.join("link")), "{touched:?}");
        assert_eq!(touched.last().unwrap(), &r.join("a/b/f"));
    }

    #[test]
    fn an_intermediate_file_link_is_touched_though_neither_end_is_inside() {
        // `live -> a/b/bridge -> outside/login`: the bridge is in `a/b`.
        let (_dir, r) = root();
        fs::create_dir_all(r.join("outside")).unwrap();
        fs::write(r.join("outside/login"), "x").unwrap();
        symlink(r.join("outside/login"), r.join("a/b/bridge")).unwrap();
        symlink(r.join("a/b/bridge"), r.join("live")).unwrap();
        let touched = trace_path(&r.join("live")).unwrap();
        assert!(touched.contains(&r.join("a/b/bridge")), "{touched:?}");
        assert!(touched.contains(&r.join("outside/login")), "{touched:?}");
        assert!(touched.iter().any(|p| p.starts_with(r.join("a/b"))));
    }

    #[test]
    fn a_linked_directory_component_is_walked_through() {
        let (_dir, r) = root();
        symlink(r.join("a/b"), r.join("dirlink")).unwrap();
        let touched = trace_path(&r.join("dirlink/f")).unwrap();
        assert!(touched.contains(&r.join("dirlink")), "{touched:?}");
        assert!(touched.contains(&r.join("a/b")), "{touched:?}");
        assert_eq!(touched.last().unwrap(), &r.join("a/b/f"));
    }

    #[test]
    fn relative_targets_are_taken_from_the_link_s_directory_and_dots_apply_to_the_resolved_prefix()
    {
        let (_dir, r) = root();
        symlink("b/f", r.join("a/rel")).unwrap();
        let touched = trace_path(&r.join("a/rel")).unwrap();
        assert_eq!(touched.last().unwrap(), &r.join("a/b/f"));
        symlink("../a/b/f", r.join("a/up")).unwrap();
        assert_eq!(
            trace_path(&r.join("a/up")).unwrap().last().unwrap(),
            &r.join("a/b/f")
        );
        // `..` after a linked directory goes up from where the link leads, not from the link.
        symlink(r.join("a/b"), r.join("dirlink")).unwrap();
        let touched = trace_path(&r.join("dirlink/../b/f")).unwrap();
        assert_eq!(touched.last().unwrap(), &r.join("a/b/f"));
    }

    #[test]
    fn a_missing_component_ends_the_walk_with_what_came_before() {
        let (_dir, r) = root();
        symlink(r.join("a/missing/deeper"), r.join("dangling")).unwrap();
        let touched = trace_path(&r.join("dangling")).unwrap();
        assert!(touched.contains(&r.join("dangling")), "{touched:?}");
        assert!(touched.contains(&r.join("a")), "{touched:?}");
        assert!(!touched.contains(&r.join("a/missing")));
        // A file where a directory is expected is also the end of the walk.
        assert!(
            trace_path(&r.join("a/b/f/x"))
                .unwrap()
                .contains(&r.join("a/b/f"))
        );
    }

    #[test]
    fn a_loop_is_an_error_and_a_relative_path_is_refused() {
        let (_dir, r) = root();
        symlink(r.join("two"), r.join("one")).unwrap();
        symlink(r.join("one"), r.join("two")).unwrap();
        assert!(trace_path(&r.join("one")).is_err());
        assert!(trace_path(Path::new("a/b")).is_err());
    }
}
