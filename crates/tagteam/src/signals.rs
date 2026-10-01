//! §14.1: SIGINT, SIGTERM and SIGHUP never stop a command where they land. Each only records
//! its number in the cancel token, and the work stops at its next cancellation point. SIGKILL
//! and SIGQUIT keep their default disposition.

use tagteam_provider::Cancel;

/// The signals a user sends to stop a command (§14.1).
const CAUGHT: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

/// Registers SIGINT, SIGTERM and SIGHUP to store their number in `cancel`'s cell (Decision 2).
/// `signal-hook`'s handler is async-signal-safe and restarts interrupted syscalls, so every
/// cancellation point polls the token rather than waiting for `EINTR`.
pub fn install(cancel: &Cancel) -> std::io::Result<()> {
    for signal in CAUGHT {
        signal_hook::flag::register_usize(signal, cancel.cell(), signal as usize)?;
    }
    Ok(())
}
