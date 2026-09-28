use std::ffi::CStr;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tagteam_provider::Env;

use crate::paths::nfc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    OAuth,
    ManagedKey,
}

/// The directory string whose hash suffixes the item, or `None` for the default items
/// (Appendix A.2: `useDefault`).
fn suffix_source(env: &Env) -> Option<String> {
    match &env.claude_securestorage_config_dir {
        Some(v) if v.is_empty() => None,
        Some(v) => Some(nfc(v)),
        None => env
            .claude_config_dir
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(nfc),
    }
}

fn service_for(kind: ItemKind, source: Option<&str>) -> String {
    let n = match kind {
        ItemKind::OAuth => "-credentials",
        ItemKind::ManagedKey => "",
    };
    match source {
        None => format!("Claude Code{n}"),
        Some(dir) => {
            let hash = hex::encode(Sha256::digest(dir.as_bytes()).as_slice());
            format!("Claude Code{n}-{}", &hash[..8])
        }
    }
}

pub fn keychain_service(env: &Env, kind: ItemKind) -> String {
    service_for(kind, suffix_source(env).as_deref())
}

/// Every item a reader tries, primary first (Appendix A.2): a symlinked directory is also read
/// as the hash of its target (a relative target joins the link's parent), and an explicitly
/// set `CLAUDE_CONFIG_DIR=~/.claude` also falls back to the unsuffixed item. Anything that
/// snapshots, clears or restores the live credential must cover all of them.
pub fn read_services(env: &Env, kind: ItemKind) -> Vec<String> {
    let mut out = vec![keychain_service(env, kind)];
    let mut push = |s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    if let Some(dir) = suffix_source(env) {
        let link = PathBuf::from(&dir);
        if let Ok(target) = std::fs::read_link(&link) {
            let target = if target.is_absolute() {
                target
            } else {
                link.parent().unwrap_or(Path::new("/")).join(target)
            };
            push(service_for(kind, Some(&nfc(target.as_os_str()))));
        }
    }
    let explicit_default = env.claude_securestorage_config_dir.is_none()
        && env.claude_config_dir.as_deref().is_some_and(|v| {
            let s = v.to_string_lossy();
            PathBuf::from(s.trim_end_matches('/')) == env.home.join(".claude")
        });
    if explicit_default {
        push(service_for(kind, None));
    }
    out
}

fn passwd_name() -> Option<String> {
    // SAFETY: getpwuid returns a pointer into static storage or null; we copy out at once.
    unsafe {
        let pw = libc::getpwuid(libc::geteuid());
        if pw.is_null() {
            return None;
        }
        CStr::from_ptr((*pw).pw_name)
            .to_str()
            .ok()
            .map(str::to_owned)
    }
}

/// `$USER`, else the passwd name, else `claude-code-user`; also `claude-code-user` when the
/// name fails `^[a-zA-Z0-9._-]+$`.
pub fn keychain_account(env: &Env) -> String {
    let name = env
        .user
        .clone()
        .filter(|u| !u.is_empty())
        .or_else(passwd_name);
    match name {
        Some(n)
            if n.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) =>
        {
            n
        }
        _ => "claude-code-user".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn env() -> Env {
        let mut e = Env::for_test(Path::new("/"));
        e.home = PathBuf::from("/home/tester");
        e
    }

    #[test]
    fn default_items_are_unsuffixed() {
        assert_eq!(
            keychain_service(&env(), ItemKind::OAuth),
            "Claude Code-credentials"
        );
        assert_eq!(
            keychain_service(&env(), ItemKind::ManagedKey),
            "Claude Code"
        );
    }

    #[test]
    fn a_config_dir_suffixes_with_the_raw_string_hash() {
        let mut e = env();
        e.claude_config_dir = Some("/home/tester/profile".into());
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-535fa96b"
        );
        assert_eq!(
            keychain_service(&e, ItemKind::ManagedKey),
            "Claude Code-535fa96b"
        );
        e.claude_config_dir = Some("/home/tester/profile/".into());
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-2c60625b"
        );
        e.claude_config_dir = Some("".into());
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials"
        );
    }

    #[test]
    fn names_are_nfc_normalized() {
        let mut e = env();
        e.claude_config_dir = Some("/home/tester/cafe\u{301}".into()); // NFD
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-3f2ee927"
        );
    }

    #[test]
    fn a_defined_secure_storage_dir_decides_alone() {
        let mut e = env();
        e.claude_config_dir = Some("/home/tester/profile".into());
        e.claude_securestorage_config_dir = Some("".into());
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials"
        );
        e.claude_config_dir = None;
        e.claude_securestorage_config_dir = Some("/home/tester/profile".into());
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-535fa96b"
        );
    }

    #[test]
    fn an_explicit_default_config_dir_reads_suffixed_then_plain() {
        let mut e = env();
        e.claude_config_dir = Some("/home/tester/.claude".into());
        assert_eq!(
            read_services(&e, ItemKind::OAuth),
            vec![
                "Claude Code-credentials-b2e2cf9d".to_string(),
                "Claude Code-credentials".into()
            ]
        );
        e.claude_config_dir = Some("/home/tester/profile".into());
        assert_eq!(read_services(&e, ItemKind::OAuth).len(), 1);
    }

    #[test]
    fn a_symlinked_config_dir_is_also_read_by_its_target() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("real");
        std::fs::create_dir(&target).unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink("real", &link).unwrap(); // relative target
        let mut e = env();
        e.claude_config_dir = Some(link.clone().into_os_string());
        let hash = |p: &Path| {
            hex::encode(Sha256::digest(p.to_str().unwrap().as_bytes()).as_slice())[..8].to_owned()
        };
        assert_eq!(
            read_services(&e, ItemKind::OAuth),
            vec![
                format!("Claude Code-credentials-{}", hash(&link)),
                format!("Claude Code-credentials-{}", hash(&target))
            ]
        );
    }

    #[test]
    fn the_account_falls_back_to_claude_code_user() {
        let mut e = env();
        assert_eq!(keychain_account(&e), "tester");
        e.user = Some("bad user".into());
        assert_eq!(keychain_account(&e), "claude-code-user");
        e.user = None;
        let name = keychain_account(&e);
        assert!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        );
    }

    #[test]
    fn the_r1_spike_hash_is_pinned() {
        let mut e = env();
        e.claude_config_dir = Some("/tmp/tagteam-r1-spike".into());
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-ba6c431d"
        );
    }
}
