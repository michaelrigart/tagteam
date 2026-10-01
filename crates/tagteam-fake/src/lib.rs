//! `FakeAgent`: a test-only provider whose shapes differ from Claude Code's on purpose (§15.2).
//! It has an identity with no email, credential kinds Claude Code does not have, one live lock,
//! and no managed-key axis. Engine tests run against it beside Claude Code, so the `Provider`
//! trait cannot quietly take on Claude Code's shape before a real second provider exists.
#![forbid(unsafe_code)]

mod paths;
mod provider;
mod shape;
mod usage;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use serde_json::{Value, json};
use tagteam_provider::Env;

pub use paths::FakePaths;
pub use provider::FakeAgent;
pub use shape::{DEVICE, KIND_STATIC, KIND_TOKEN, KINDS};

pub const FAKE_AGENT: &str = "fake-agent";

/// The access-token expiry `login` writes: far enough out that nothing freshens it unless a
/// test says so.
pub const LOGIN_EXPIRES: i64 = 4_102_444_800_000;

/// A stored or live credential. `renew` makes it a refreshable `fa_token`, and without it the
/// credential is an `fa_static`. `device` is the machine-shared key.
pub fn credential_json(token: &str, renew: Option<&str>, expires: Option<i64>) -> Value {
    let mut fa = serde_json::Map::new();
    fa.insert("token".into(), json!(token));
    if let Some(renew) = renew {
        fa.insert("renew".into(), json!(renew));
    }
    if let Some(expires) = expires {
        fa.insert("expires".into(), json!(expires));
    }
    json!({"fa": Value::Object(fa), "device": {"id": "machine-shared"}})
}

/// A FakeAgent identity object. It has no email anywhere.
pub fn identity_json(handle: &str, workspace: &str, uid: &str) -> Value {
    json!({"handle": handle, "workspace": workspace, "uid": uid})
}

/// What logging in to FakeAgent leaves behind: `identity.json` with the login and an
/// unrelated `prefs` key, and the credential file at mode 0600. A test helper that panics on
/// I/O failure.
pub fn login(env: &Env, handle: &str, workspace: &str, token: &str, renew: &str) {
    let p = FakePaths::resolve(env);
    fs::create_dir_all(&p.dir).unwrap();
    let doc = json!({
        "identity": identity_json(handle, workspace, &format!("uid-{handle}")),
        "prefs": {"theme": "x"}
    });
    fs::write(
        &p.identity,
        format!("{}\n", serde_json::to_string_pretty(&doc).unwrap()),
    )
    .unwrap();
    let cred = credential_json(token, Some(renew), Some(LOGIN_EXPIRES));
    fs::write(&p.credential, serde_json::to_vec(&cred).unwrap()).unwrap();
    fs::set_permissions(&p.credential, fs::Permissions::from_mode(0o600)).unwrap();
}
