//! The `tagteam daemon` command run in this test process, with a socket bound. A test binary
//! of its own because `bind_private` narrows the umask around its bind, and the umask belongs
//! to the whole process (the same reason as `tagteam-daemon/tests/umask.rs`). Next to the
//! other tests of `app.rs`, a directory one of their threads made while the mask was narrowed
//! came out 0600, and the next file made in it failed with EACCES. No other test lives in this
//! binary, so none creates anything while the daemon binds. Daemons that tests run as
//! processes (`daemon_cli.rs` and the rest) have a umask of their own and are safe.

mod common;

use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use common::seed_home;
use tagteam::app::{self, Context, Io};
use tagteam::cli::Cli;
use tagteam::prompt::Prompter;
use tagteam_cc::live::Platform;
use tagteam_provider::{Cancel, Env, FakeKeychain};

/// A prompter for a command with nobody to ask: any question is a failure of the test.
struct NoTerminal;

impl Prompter for NoTerminal {
    fn interactive(&self) -> bool {
        false
    }
    fn confirm(&mut self, q: &str, _default_yes: bool) -> bool {
        panic!("unexpected prompt: {q}")
    }
    fn choose(&mut self, q: &str, _o: &[String]) -> Option<usize> {
        panic!("unexpected prompt: {q}")
    }
    fn secret(&mut self, q: &str) -> Option<String> {
        panic!("unexpected prompt: {q}")
    }
}

#[test]
fn daemon_without_a_terminal_runs_no_keychain_check_and_a_signal_stops_it_with_exit_0() {
    // D4.1 step 3: a service has nobody to answer, and the check could block on a dialog
    // (Appendix A.3). With no store the provider is idle. The signal ends the run at its next
    // slice, with no "too late" notice.
    let dir = common::short_tempdir();
    let env = Env::for_test(dir.path());
    seed_home(&env);
    let kc = Arc::new(FakeKeychain::new());
    kc.set_locked(true);
    let cancel = Cancel::new();
    let signal = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(1));
            cancel.request(libc::SIGTERM);
        })
    };
    let mut env = env;
    env.cancel = cancel.clone();
    let ctx = Context {
        env,
        keychain: kc.clone(),
        vault_keychain: None,
        platform: Platform::MacOs,
        api_base: Some(common::OFFLINE_API_BASE.into()),
        stdout_terminal: false,
        no_color_env: false,
        force_color_env: false,
        shown_warnings: Arc::from(Vec::<String>::new()),
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = app::run(
        Cli::try_parse_from(["tagteam", "daemon"]).unwrap(),
        ctx,
        &mut Io {
            out: &mut out,
            err: &mut err,
            prompter: &mut NoTerminal,
        },
    );
    signal.join().unwrap();
    let (out, err) = (
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    );
    assert_eq!((code, err.as_str()), (0, ""), "{out}");
    let lines: Vec<&str> = out.lines().map(|l| &l[10..]).collect();
    let started = format!(
        "daemon started (pid {}); Ctrl-C stops it",
        std::process::id()
    );
    assert_eq!(lines.first(), Some(&started.as_str()), "{out}");
    assert!(lines.contains(&"mode idle"), "{out}");
    assert_eq!(lines.last(), Some(&"stopped"), "{out}");
    assert_eq!(kc.unlock_attempts(), 0);
    // `lock_state` never prompts on the fake, but a real one can block on SecurityAgent.
    assert_eq!(
        kc.lock_state_calls(),
        0,
        "the daemon probed the keychain's lock state"
    );
}
