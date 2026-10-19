use std::ffi::{OsStr, OsString};

use tagteam_provider::Env;

pub const CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";
pub const SECURE_STORAGE_DIR: &str = "CLAUDE_SECURESTORAGE_CONFIG_DIR";

fn get<'e>(env: &'e Env, name: &str) -> Option<&'e OsStr> {
    env.homes.get(name).map(OsString::as_os_str)
}

fn set(env: &mut Env, name: &str, value: Option<OsString>) {
    match value {
        Some(v) => env.homes.insert(name.to_owned(), v),
        None => env.homes.remove(name),
    };
}

/// `CLAUDE_CONFIG_DIR`: the raw string, never canonicalized; `None` when undefined.
pub fn config_dir(env: &Env) -> Option<&OsStr> {
    get(env, CONFIG_DIR)
}

/// `CLAUDE_SECURESTORAGE_CONFIG_DIR`: `None` when undefined, `Some("")` when defined but empty
/// (Appendix A.1).
pub fn securestorage_dir(env: &Env) -> Option<&OsStr> {
    get(env, SECURE_STORAGE_DIR)
}

/// `None` removes the name.
pub fn set_config_dir(env: &mut Env, value: Option<OsString>) {
    set(env, CONFIG_DIR, value);
}

/// `None` removes the name.
pub fn set_securestorage_dir(env: &mut Env, value: Option<OsString>) {
    set(env, SECURE_STORAGE_DIR, value);
}
