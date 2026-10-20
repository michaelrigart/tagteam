use std::io::Write;

use serde_json::{Value, json};
use tagteam_daemon::daemon::DaemonError;
use tagteam_engine::EngineError;
use tagteam_engine::displace::PurgeError;

use crate::displaced_cmd;

use super::Ended;
use super::context::Io;

/// §13.1.
pub(crate) const EXIT_ERROR: i32 = 1;
pub(crate) const EXIT_USAGE: i32 = 2;

/// `error.type` kinds the CLI raises itself; the engine's come from `EngineError::kind`.
pub(crate) const KIND_USAGE: &str = "usage";
pub(crate) const KIND_ROOT: &str = "root";
pub(super) const KIND_KEYCHAIN_LOCKED: &str = "keychain-locked";
const KIND_CANCELLED: &str = "cancelled";
pub(super) const KIND_INVALID_INPUT: &str = "invalid-input";
pub(super) const KIND_UNMANAGED_ACCOUNT: &str = "unmanaged-account";
/// The engine's kind for the same condition; the CLI raises it with its own message.
pub(super) const KIND_NO_LIVE_LOGIN: &str = "no-live-login";
/// A provider without the capability a command needs (§4.5).
pub(super) const KIND_UNSUPPORTED: &str = "unsupported";
/// §11.1: every provider `auto` would drive has its engine running in another process.
pub(super) const KIND_ENGINE_RUNNING: &str = "engine-running";
/// `auto` found no provider with two switchable accounts.
pub(super) const KIND_NO_CANDIDATES: &str = "no-candidates";
/// §14.1, Decision 4: every interruption, whichever carrier holds its signal.
pub(crate) const KIND_INTERRUPTED: &str = "interrupted";
/// A deletion that needs a person's yes, or `--yes` (§6.3).
pub(super) const KIND_NEEDS_CONFIRMATION: &str = "needs-confirmation";
/// §13.3: an export's passphrase, which only a terminal can type. The engine's
/// `TransferError::NeedsPassphrase` has the same kind for an import's.
pub(super) const KIND_NEEDS_PASSPHRASE: &str = "needs-passphrase";
/// §13.3: every account was broken, or there was none to export.
pub(super) const KIND_NOTHING_EXPORTED: &str = "nothing-exported";
pub(super) const KIND_IO: &str = "io";
/// `daemon install` and `uninstall`: the service manager refused, or gave no answer (D9.4).
pub(super) const KIND_SERVICE: &str = "service";
/// D9.3: no `launchctl` or `systemctl` to load the unit with.
pub(super) const KIND_NO_SERVICE_MANAGER: &str = "no-service-manager";
/// `daemon status`: the daemon's socket answered something unusable, or nothing in time.
pub(super) const KIND_DAEMON: &str = "daemon";
/// `--expect-account` named an account other than the one the reference resolved to (§10.4).
pub const KIND_STALE_SELECTION: &str = "stale-selection";

const CANCELLED: &str = "cancelled";

pub(super) enum Failure {
    Engine(EngineError),
    /// A `--purge` that stopped part way: the entries deleted before the failure are reported
    /// beside it (§6.3).
    Purge(PurgeError),
    Usage(String),
    Message(&'static str, String),
}

impl From<EngineError> for Failure {
    fn from(e: EngineError) -> Self {
        Failure::Engine(e)
    }
}

impl From<PurgeError> for Failure {
    fn from(e: PurgeError) -> Self {
        if e.deleted.is_empty() {
            Failure::Engine(e.cause)
        } else {
            Failure::Purge(e)
        }
    }
}

impl From<DaemonError> for Failure {
    fn from(e: DaemonError) -> Self {
        match e {
            DaemonError::Engine(e) => Failure::Engine(e),
        }
    }
}

impl Failure {
    /// Writes the failure and says how the command ended. With `signals`, one a signal caused is
    /// an interruption, which `run` reports; without, it is an error like any other
    /// (`statusline`'s).
    pub(super) fn report(self, io: &mut Io<'_>, json: bool, signals: bool) -> Ended {
        match self {
            Failure::Engine(e) => match e.signal().filter(|_| signals) {
                Some(signal) => Ended::Interrupted(signal),
                None => Ended::Code(io.fail(json, e.kind(), &e.to_string())),
            },
            Failure::Purge(e) => match e.signal().filter(|_| signals) {
                Some(signal) => Ended::Interrupted(signal),
                None => Ended::Code(fail_purge(io, json, &e)),
            },
            Failure::Message(kind, m) => Ended::Code(io.fail(json, kind, &m)),
            Failure::Usage(m) => {
                io.fail(json, KIND_USAGE, &m);
                Ended::Code(EXIT_USAGE)
            }
        }
    }
}

pub(super) fn cancelled() -> Failure {
    Failure::Message(KIND_CANCELLED, CANCELLED.into())
}

/// §13.2: the one object `--json` prints for any error.
pub(crate) fn error_json(kind: &str, message: &str) -> Value {
    json!({"schemaVersion": 1, "error": {"type": kind, "message": message}})
}

/// `error_json` for a purge that stopped part way, with `deleted` added beside `error`.
fn purge_error_json(kind: &str, message: &str, deleted: &[String]) -> Value {
    let mut v = error_json(kind, message);
    v["deleted"] = json!(deleted);
    v
}

/// `error_json` on `out` under `--json`, else `tagteam: message` on `err`: the one error form.
/// `Io::fail` writes through it, and so do the sites that run before an `Io` exists.
pub(crate) fn fail_to(
    out: &mut dyn Write,
    err: &mut dyn Write,
    json: bool,
    kind: &str,
    message: &str,
) -> i32 {
    if json {
        let _ = writeln!(out, "{}", error_json(kind, message));
    } else {
        let _ = writeln!(err, "tagteam: {message}");
    }
    EXIT_ERROR
}

/// A purge's failure after it deleted something: the human form lists each deletion before the
/// error, and `--json` carries them as `deleted` beside `error`.
fn fail_purge(io: &mut Io<'_>, json: bool, e: &PurgeError) -> i32 {
    if json {
        let _ = writeln!(
            io.out,
            "{}",
            purge_error_json(e.kind(), &e.to_string(), &e.deleted)
        );
    } else {
        let _ = write!(io.out, "{}", displaced_cmd::purged_human(&e.deleted));
        let _ = writeln!(io.err, "tagteam: {e}");
    }
    EXIT_ERROR
}
