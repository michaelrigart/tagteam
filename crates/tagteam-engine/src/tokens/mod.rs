//! The token report: the index of a provider's transcripts, the evidence that places their
//! sessions, and the report over them.

mod evidence;
mod reader;
mod refresh;
mod view;

use std::path::PathBuf;

use tagteam_core::{Fingerprint, ProviderId};
use tagteam_provider::Env;

pub use evidence::*;
pub use refresh::*;
pub use view::*;

/// `locks/tokens-<provider>.lock` (spec §5.1): held by one refresh of the provider at a time, a
/// leaf. A provider ID that is not a plain name (anything outside `[A-Za-z0-9._-]`, or one
/// starting with `.`) cannot name a path: its lock is named by the first 12 hex digits of the
/// ID's SHA-256, inside `locks/`, as `engine_lock_path` names an engine lock.
pub fn tokens_lock_path(env: &Env, provider: &ProviderId) -> PathBuf {
    let id = provider.as_str();
    let plain = !id.is_empty()
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    let name = if plain {
        format!("tokens-{id}.lock")
    } else {
        format!(
            "tokens-{}.lock",
            Fingerprint::of_secret(id.as_bytes()).short12()
        )
    };
    env.data_dir().join("locks").join(name)
}

#[cfg(test)]
mod tests {
    use tagteam_core::ProviderId;
    use tagteam_provider::Env;

    use super::*;

    #[test]
    fn the_tokens_lock_is_in_locks_and_a_stored_string_never_escapes_it() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::for_test(d.path());
        let locks = env.data_dir().join("locks");
        assert_eq!(
            tokens_lock_path(&env, &ProviderId::new("claude-code")),
            locks.join("tokens-claude-code.lock")
        );
        let hostile = tokens_lock_path(&env, &ProviderId::new("/../../escape"));
        assert_eq!(hostile.parent(), Some(locks.as_path()));
        let name = hostile.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("tokens-") && name.ends_with(".lock") && !name.contains("escape"),
            "{name}"
        );
        assert_eq!(name.len(), "tokens-".len() + 12 + ".lock".len(), "{name}");
    }
}
