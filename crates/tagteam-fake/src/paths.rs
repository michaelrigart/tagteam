use std::path::PathBuf;

use tagteam_provider::Env;

/// FakeAgent's home variable: a profile is a directory it names (§4.5 `session_dir_var`).
pub(crate) const HOME_VAR: &str = "FAKEAGENT_HOME";

/// Where FakeAgent keeps its state: `$FAKEAGENT_HOME` when set and non-empty, else
/// `<home>/.fakeagent/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakePaths {
    pub dir: PathBuf,
    /// `{"identity": {...}, ...}`. Only the `identity` key is FakeAgent's login.
    pub identity: PathBuf,
    /// The live credential, mode 0600.
    pub credential: PathBuf,
    /// FakeAgent's one live lock, a `mkdir` lock.
    pub lock: PathBuf,
}

impl FakePaths {
    pub fn resolve(env: &Env) -> Self {
        let dir = match env.var(HOME_VAR).filter(|v| !v.is_empty()) {
            Some(v) => PathBuf::from(v),
            None => env.home.join(".fakeagent"),
        };
        let dir = env.guard(dir);
        Self {
            identity: dir.join("identity.json"),
            credential: dir.join("credential.json"),
            lock: dir.join(".live.lock"),
            dir,
        }
    }
}
