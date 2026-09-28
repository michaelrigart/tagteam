use std::path::Path;

const MARKERS: [&str; 5] = ["docker", "lxc", "containerd", "kubepods", "overlay"];

fn mentions_container(text: &str) -> bool {
    MARKERS.iter().any(|m| text.contains(m))
}

fn in_container() -> bool {
    std::env::var_os("CONTAINER").is_some()
        || std::env::var_os("container").is_some()
        || Path::new("/.dockerenv").exists()
        || ["/proc/1/cgroup", "/proc/self/mountinfo"]
            .iter()
            .any(|p| std::fs::read_to_string(p).is_ok_and(|s| mentions_container(&s)))
}

/// §5: refuse to run as root outside a container, so no root-owned files land in the user's
/// directories (cswap's heuristics).
pub fn refuse_root() -> Result<(), String> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let root = unsafe { libc::geteuid() } == 0;
    if root && !in_container() {
        return Err(
            "tagteam refuses to run as root outside a container; run it as your own user".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::mentions_container;

    #[test]
    fn container_markers_are_recognised() {
        assert!(mentions_container("0::/kubepods/besteffort/pod1"));
        assert!(mentions_container("overlay / overlay rw"));
        assert!(!mentions_container("0::/user.slice/user-1000.slice"));
    }
}
