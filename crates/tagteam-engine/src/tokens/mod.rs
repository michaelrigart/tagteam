//! The token report: the index of a provider's transcripts, the evidence that places their
//! sessions, and the report over them.

mod evidence;
mod reader;
mod refresh;
mod view;

use std::path::PathBuf;

use tagteam_core::ProviderId;
use tagteam_provider::Env;

use crate::auto::lock_file_name;

pub use evidence::*;
pub use refresh::*;
pub use view::*;

/// `locks/tokens-<provider>.lock` (spec §5.1): held by one refresh of the provider at a time, a
/// leaf. A provider ID that is not a plain name (anything outside `[A-Za-z0-9._-]`, or one
/// starting with `.`) cannot name a path: its lock is named by the first 12 hex digits of the
/// ID's SHA-256, inside `locks/`, by the same `lock_file_name` as an engine lock.
pub fn tokens_lock_path(env: &Env, provider: &ProviderId) -> PathBuf {
    let name = lock_file_name(provider.as_str(), "tokens", "tokens");
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
