//! D5.1: `bind` narrows the umask only around its own bind. The umask belongs to the whole
//! process, so this is a test binary of its own: no other test creates a file, or binds, while
//! it reads the mask back.

mod common;

use std::path::Path;

use tagteam_daemon::server::Server;
use tagteam_provider::Cancel;

#[test]
fn bind_puts_back_the_umask_it_found() {
    let dir = common::short_tempdir();
    // SAFETY: umask(2) only swaps the process's mask, and nothing else runs in this binary.
    let found = unsafe { libc::umask(0o022) };
    let server = Server::bind(
        &dir.path().join("daemon.sock"),
        Path::new(common::HOME),
        common::board(),
        Cancel::new(),
    )
    .unwrap();
    // SAFETY: as above.
    let after = unsafe { libc::umask(found) };
    server.shutdown();
    assert_eq!(after, 0o022);
}
