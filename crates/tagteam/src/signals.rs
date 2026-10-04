//! §14.1: SIGINT, SIGTERM and SIGHUP never stop a command where they land. Each only records
//! its number in the cancel token, and the work stops at its next cancellation point. SIGKILL
//! keeps its default disposition, and so does SIGQUIT until `run` spawns `claude`
//! (`survive_quit`).

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use tagteam_provider::Cancel;

/// The signals a user sends to stop a command (§14.1).
const CAUGHT: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

/// Registers SIGINT, SIGTERM and SIGHUP to store their number in `cancel`'s cell (Decision 2).
/// `signal-hook`'s handler is async-signal-safe and restarts interrupted syscalls, so every
/// cancellation point polls the token rather than waiting for `EINTR`. A signal that was
/// ignored at startup stays ignored (`nohup tagteam ...`, a background job), as for any
/// well-behaved program.
pub fn install(cancel: &Cancel) -> std::io::Result<()> {
    for signal in CAUGHT {
        if ignored(signal) {
            continue;
        }
        signal_hook::flag::register_usize(signal, cancel.cell(), signal as usize)?;
    }
    Ok(())
}

/// Whether `signal` is set to be ignored (`SIG_IGN`) right now.
fn ignored(signal: libc::c_int) -> bool {
    // SAFETY: an all-zero `sigaction` is a valid value to be overwritten.
    let mut current: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: a null new action only queries; `current` is writable for one `sigaction`.
    let rc = unsafe { libc::sigaction(signal, std::ptr::null(), &mut current) };
    rc == 0 && current.sa_sigaction == libc::SIG_IGN
}

/// Decision 1, §12.5: while `claude` runs, Ctrl-\ is the child's. tagteam survives it through a
/// handler that stores into a cell nothing reads. A handled signal is reset to its default when
/// `claude` is exec'd, so the child keeps Ctrl-\'s default action, which `SIG_IGN` would have
/// taken from it. A SIGQUIT ignored at startup stays ignored, for both (as in `install`).
pub(crate) fn survive_quit() -> std::io::Result<()> {
    if ignored(libc::SIGQUIT) {
        return Ok(());
    }
    signal_hook::flag::register_usize(
        libc::SIGQUIT,
        Arc::new(AtomicUsize::new(0)),
        libc::SIGQUIT as usize,
    )?;
    Ok(())
}
