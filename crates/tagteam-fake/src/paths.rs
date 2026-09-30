use std::path::PathBuf;

use tagteam_provider::Env;

/// Where FakeAgent keeps its state: `<home>/.fakeagent/`.
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
        let dir = env.guard(env.home.join(".fakeagent"));
        Self {
            identity: dir.join("identity.json"),
            credential: dir.join("credential.json"),
            lock: dir.join(".live.lock"),
            dir,
        }
    }
}
