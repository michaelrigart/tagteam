const REFUSAL: &str = "tagteam refuses to run as root outside a container; run it as your own user";

/// §5: refuse to run as root outside a container, so no root-owned files land in the user's
/// directories.
pub fn refuse_root() -> Result<(), String> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    check(euid, || {
        std::fs::read_to_string("/proc/self/mountinfo").is_ok_and(|m| root_is_overlay(&m))
    })
}

/// The refusal for effective user `euid`. `in_container` is consulted only for root.
fn check(euid: libc::uid_t, in_container: impl FnOnce() -> bool) -> Result<(), String> {
    if euid == 0 && !in_container() {
        return Err(REFUSAL.into());
    }
    Ok(())
}

/// A container's root filesystem is an overlay: the `/` mount in `/proc/self/mountinfo` has
/// filesystem type `overlay`. Only that line counts. A host that runs Docker has overlay
/// mounts elsewhere, and its own root is still not one.
fn root_is_overlay(mountinfo: &str) -> bool {
    // The visible `/` is the last mount on it.
    mountinfo.lines().filter_map(root_fs_type).next_back() == Some("overlay")
}

/// For the line of a mount on `/`, its filesystem type. A line reads
/// `36 35 98:0 / / rw,relatime shared:1 - overlay overlay rw,…`: the fifth field is the mount
/// point, and the type follows the lone `-` that ends the optional fields.
fn root_fs_type(line: &str) -> Option<&str> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.get(4) != Some(&"/") {
        return None;
    }
    let separator = 6 + fields.get(6..)?.iter().position(|f| *f == "-")?;
    fields.get(separator + 1).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTAINER: &str = "\
600 500 0:50 / / rw,relatime master:1 - overlay overlay rw,lowerdir=/l,upperdir=/u
601 600 0:51 / /proc rw,nosuid,nodev,noexec - proc proc rw
";

    /// A host running Docker: its root is ext4; a container's overlay is mounted elsewhere, and
    /// "docker" and "overlay" appear all over the file.
    const DOCKER_HOST: &str = "\
22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw,errors=remount-ro
500 22 0:50 / /var/lib/docker/overlay2/abc/merged rw,relatime shared:2 - overlay overlay rw
";

    #[test]
    fn only_an_overlay_root_mount_is_a_container() {
        assert!(root_is_overlay(CONTAINER));
        assert!(!root_is_overlay(DOCKER_HOST));
        assert!(!root_is_overlay(""));
        // No optional fields: the separator follows the mount options directly.
        assert!(root_is_overlay("1 0 0:1 / / rw - overlay overlay rw\n"));
        // A later mount on `/` hides the earlier one.
        assert!(!root_is_overlay(&format!(
            "{CONTAINER}700 600 8:1 / / rw - ext4 /dev/sda1 rw\n"
        )));
    }

    #[test]
    fn root_is_refused_outside_a_container() {
        assert_eq!(check(0, || false), Err(REFUSAL.to_owned()));
        assert_eq!(check(0, || true), Ok(()));
        assert_eq!(
            check(501, || unreachable!("never consulted for a normal user")),
            Ok(())
        );
    }
}
