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
pub(crate) fn suffix_source(env: &Env) -> Option<String> {
    match crate::env::securestorage_dir(env) {
        Some(v) if v.is_empty() => None,
        Some(v) => Some(nfc(v)),
        None => crate::env::config_dir(env)
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

/// `$USER`, else the passwd name, else `claude-code-user`; also `claude-code-user` when the
/// name fails `^[a-zA-Z0-9._-]+$`.
pub fn keychain_account(env: &Env) -> String {
    let name = env
        .user
        .clone()
        .filter(|u| !u.is_empty())
        .or_else(tagteam_provider::passwd::user_name);
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
    fn the_live_item_spelling_is_what_its_name_hashes() {
        // Appendix A.2, through the `Provider` method purge compares an orphan's spelling with.
        use tagteam_provider::Provider;

        let cc = crate::ClaudeCode::new(
            std::sync::Arc::new(tagteam_provider::FakeKeychain::new()),
            crate::live::Platform::MacOs,
        );
        let mut e = env();
        assert_eq!(cc.live_item_spelling(&e), None, "the default items");
        crate::env::set_config_dir(&mut e, Some("/cfg/e\u{301}".into()));
        assert_eq!(
            cc.live_item_spelling(&e).as_deref(),
            Some("/cfg/\u{e9}"),
            "NFC"
        );
        crate::env::set_securestorage_dir(&mut e, Some("/secure".into()));
        assert_eq!(
            cc.live_item_spelling(&e).as_deref(),
            Some("/secure"),
            "first"
        );
        crate::env::set_securestorage_dir(&mut e, Some("".into()));
        assert_eq!(cc.live_item_spelling(&e), None, "empty counts as unset");
        crate::env::set_securestorage_dir(&mut e, None);
        crate::env::set_config_dir(&mut e, Some("".into()));
        assert_eq!(cc.live_item_spelling(&e), None);
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
        crate::env::set_config_dir(&mut e, Some("/home/tester/profile".into()));
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-535fa96b"
        );
        assert_eq!(
            keychain_service(&e, ItemKind::ManagedKey),
            "Claude Code-535fa96b"
        );
        crate::env::set_config_dir(&mut e, Some("/home/tester/profile/".into()));
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-2c60625b"
        );
        crate::env::set_config_dir(&mut e, Some("".into()));
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials"
        );
    }

    #[test]
    fn names_are_nfc_normalized() {
        let mut e = env();
        crate::env::set_config_dir(&mut e, Some("/home/tester/cafe\u{301}".into())); // NFD
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-3f2ee927"
        );
    }

    /// NFC follows the Unicode tables: U+1AEF, unassigned in Unicode 17, has combining class 220
    /// in Unicode 18 and so sorts before U+0301. New tables would rename an item Claude Code named
    /// with older ones, so `unicode-normalization` is pinned (Cargo.toml).
    #[test]
    fn names_use_the_pinned_unicode_tables() {
        let mut e = env();
        crate::env::set_config_dir(&mut e, Some("/tmp/x\u{301}\u{1aef}".into()));
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-4a00d355"
        );
    }

    #[test]
    fn a_defined_secure_storage_dir_decides_alone() {
        let mut e = env();
        crate::env::set_config_dir(&mut e, Some("/home/tester/profile".into()));
        crate::env::set_securestorage_dir(&mut e, Some("".into()));
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials"
        );
        crate::env::set_config_dir(&mut e, None);
        crate::env::set_securestorage_dir(&mut e, Some("/home/tester/profile".into()));
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-535fa96b"
        );
    }

    #[test]
    fn an_explicit_default_config_dir_names_only_its_suffixed_items() {
        // Appendix A.2 (2.1.286): no fallback to the unsuffixed items for `~/.claude`.
        let mut e = env();
        crate::env::set_config_dir(&mut e, Some("/home/tester/.claude".into()));
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-b2e2cf9d"
        );
        assert_eq!(
            keychain_service(&e, ItemKind::ManagedKey),
            "Claude Code-b2e2cf9d"
        );
    }

    #[test]
    fn a_symlinked_config_dir_is_named_by_the_link_s_spelling_alone() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("real");
        std::fs::create_dir(&target).unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink("real", &link).unwrap();
        let mut e = env();
        crate::env::set_config_dir(&mut e, Some(link.clone().into_os_string()));
        let hash = |p: &Path| {
            hex::encode(Sha256::digest(p.to_str().unwrap().as_bytes()).as_slice())[..8].to_owned()
        };
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            format!("Claude Code-credentials-{}", hash(&link))
        );
        assert_ne!(hash(&link), hash(&target));
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
        crate::env::set_config_dir(&mut e, Some("/tmp/tagteam-r1-spike".into()));
        assert_eq!(
            keychain_service(&e, ItemKind::OAuth),
            "Claude Code-credentials-ba6c431d"
        );
    }
}
