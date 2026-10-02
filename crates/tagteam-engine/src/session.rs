//! §12.8: where a process stands, inside a run shell or not. It is found once, before the
//! engine is built (Decision 6), so every engine read of `env` already sees the default home.

use std::path::Path;

use tagteam_provider::profile::{MARKER_FILE, ProfileMarker, RunShell};
use tagteam_provider::{Env, Read};

use crate::registry::ProviderRegistry;

fn unreadable_marker(dir: &Path, env: &Env, detail: String) -> (RunShell, Env) {
    (
        RunShell::Unreadable {
            marker: dir.join(MARKER_FILE),
            detail,
        },
        env.clone(),
    )
}

/// §12.8: the first registered provider with sessions whose `session_dir(env)` holds a marker
/// decides. Returns the run shell and the effective `Env` (the marker provider's
/// `apply_outer_home`, or `env` unchanged when `Outside` or `Unreadable`). The provider whose
/// variable found the marker must be the provider the marker names; a mismatch is `Unreadable`.
///
/// Only the marker is consulted (B.57): not where the directory lies, and not `XDG_DATA_HOME`,
/// which a run shell may have changed. A marker that names another provider leaves this
/// provider's outer home unknown, and so does an `outer` record the provider cannot apply, so
/// both count as unreadable.
pub fn detect_run_shell(env: &Env, registry: &ProviderRegistry) -> (RunShell, Env) {
    for p in registry.all() {
        if !p.capabilities().sessions {
            continue;
        }
        let Some(dir) = p.session_dir(env) else {
            continue;
        };
        let marker = match ProfileMarker::read(&dir) {
            Read::Absent => continue,
            Read::Unreadable(e) => return unreadable_marker(&dir, env, e.detail),
            Read::Present(m) => m,
        };
        if marker.provider != p.id() {
            let detail = format!("it names the provider {}, not {}", marker.provider, p.id());
            return unreadable_marker(&dir, env, detail);
        }
        return match p.apply_outer_home(env, &marker.outer) {
            Ok(outer) => (
                RunShell::Inside {
                    profile: dir,
                    marker,
                },
                outer,
            ),
            Err(e) => unreadable_marker(&dir, env, e.to_string()),
        };
    }
    (RunShell::Outside, env.clone())
}
