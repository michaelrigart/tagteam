//! The checks that need the user's own session, run only when asked for (§15.4): `--ssh HOST`
//! and `--locked-keychain`. Both are macOS's, and both run in the live phase, where tagteam has
//! written the default home's item.

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_cc::ItemKind;
use tagteam_provider::LockState;

use super::profile::read_name;
use super::{ask, read_json};
use crate::compat::ctx::Ctx;
use crate::compat::keychain::{LOGIN_KEYCHAIN, SECURITY, login_lock_state};
use crate::compat::report::{Outcome, Probe};
use crate::compat::sys::{HarnessError, cancel, harness, shell_quote, wait_interactive};

/// The `<word> <number>...` lines the SSH script writes.
fn field(results: &str, word: &str, i: usize) -> Option<i64> {
    results.lines().find_map(|l| {
        l.strip_prefix(word)?
            .split_whitespace()
            .nth(i)?
            .parse()
            .ok()
    })
}

/// §17 R1: over SSH the login keychain stays locked until `security unlock-keychain`; once it
/// is unlocked, `claude` reads the item tagteam wrote at activation without any prompt, and so
/// does `security`. HOST must be this Mac: the script works in this run's scratch directory.
pub fn ssh_keychain_read(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let host = ctx
        .ssh
        .clone()
        .ok_or_else(|| harness("ssh-keychain-read needs --ssh HOST"))?;
    let live = ctx.live();
    let item = ctx.item(&live, ItemKind::OAuth)?;
    p.expect(
        "tagteam wrote the default home's item",
        item.exists().is_present(),
        json!(item.service),
    );
    let vars = ctx.vars(&live);
    ctx.roots.check_env(&vars)?;
    let env: Vec<String> = vars
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                k.to_string_lossy(),
                shell_quote(&v.to_string_lossy())
            )
        })
        .collect();
    let out = ctx.layout.scratch.join("ssh-results");
    let json_out = ctx.layout.scratch.join("ssh-auth-status.json");
    let q = |p: &std::path::Path| shell_quote(&p.to_string_lossy());
    let script = format!(
        "cd {work} || exit 90; \
         {security} show-keychain-info >/dev/null 2>&1; echo \"locked $?\" > {out}; \
         {security} unlock-keychain \"$HOME/{login}\"; echo \"unlock $?\" >> {out}; \
         start=$(date +%s); env -i {env} {claude} auth status --json > {json} 2>/dev/null; \
         echo \"auth $? $(( $(date +%s) - start ))\" >> {out}; \
         {security} find-generic-password -a {account} -w -s {service} >/dev/null 2>&1; \
         echo \"read $?\" >> {out}",
        work = q(&ctx.layout.work()),
        security = SECURITY,
        out = q(&out),
        login = LOGIN_KEYCHAIN,
        env = env.join(" "),
        claude = q(&ctx.claude),
        json = q(&json_out),
        account = shell_quote(&item.account),
        service = shell_quote(&item.service),
    );
    eprintln!("cargo xtask compat: over SSH to {host}; enter your login password when asked.");
    // A cancellation point (`wait_interactive`): a signal ends ssh, and the run unwinds.
    let mut ssh = Command::new("ssh")
        .args(["-t", &host, &script])
        .spawn()
        .map_err(|e| harness(format!("could not run ssh: {e}")))?;
    let status = wait_interactive(&mut ssh, cancel())?;
    if status.code() == Some(90) {
        return Err(harness(format!(
            "{host} is not this Mac: the scratch directory is not there"
        )));
    }
    let results = fs::read_to_string(&out).unwrap_or_default();
    p.note(
        "over SSH, the lock check before unlocking (36: locked)",
        json!(field(&results, "locked", 0)),
    );
    p.expect_eq(
        "unlocked over SSH",
        json!(0),
        json!(field(&results, "unlock", 0)),
    );
    let v = read_json(&json_out).unwrap_or(Value::Null);
    p.expect_eq(
        "claude read the item: loggedIn",
        true,
        v["loggedIn"].clone(),
    );
    p.expect_eq(
        "claude read the item: authMethod",
        "claude.ai",
        v["authMethod"].clone(),
    );
    p.expect(
        "without a prompt: auth status took under 10 s",
        field(&results, "auth", 1).is_some_and(|s| s < 10),
        json!(field(&results, "auth", 1)),
    );
    p.expect_eq(
        "security reads the item tagteam wrote",
        json!(0),
        json!(field(&results, "read", 0)),
    );
    Ok(p.finish("CC read tagteam's item over SSH without a prompt"))
}

/// The login keychain as `locked-login-keychain` drives it: the real one through
/// `/usr/bin/security`, or a test's double.
pub trait LoginKeychain {
    fn state(&self) -> LockState;
    fn lock(&self) -> Result<(), HarnessError>;
    /// Asks for the password on the terminal, as `security unlock-keychain` does.
    fn unlock(&self) -> Result<(), HarnessError>;
}

/// The user's login keychain file.
pub struct Login(pub PathBuf);

impl LoginKeychain for Login {
    fn state(&self) -> LockState {
        login_lock_state(&self.0)
    }

    fn lock(&self) -> Result<(), HarnessError> {
        // Bounded (`lock-keychain` asks for nothing), so not a cancellation point.
        let locked = Command::new(SECURITY)
            .arg("lock-keychain")
            .arg(&self.0)
            .status()?;
        if locked.success() {
            Ok(())
        } else {
            Err(harness("security lock-keychain failed"))
        }
    }

    fn unlock(&self) -> Result<(), HarnessError> {
        // Restoration: it asks for the password and must run after a cancel too, so it is
        // deliberately not a cancellation point.
        eprintln!("cargo xtask compat: unlocking your login keychain.");
        let unlocked = Command::new(SECURITY)
            .arg("unlock-keychain")
            .arg(&self.0)
            .status()?;
        if unlocked.success() {
            Ok(())
        } else {
            Err(harness("security unlock-keychain failed"))
        }
    }
}

/// What `with_login_locked` did.
#[derive(Debug, PartialEq)]
pub enum Locked<T> {
    /// It ran the body, and put the keychain back as it found it.
    Ran(T),
    /// The keychain's state was `Unknown`, so it changed nothing and ran nothing.
    Unknown,
}

/// Why `locked-login-keychain` did not run: the outcome a check that cannot run gets.
pub const UNKNOWN_STATE: &str = "the login keychain's lock state is unknown, as it is while a dialog waits for an answer (Appendix A.3): answer or dismiss the dialog, then run the check again; nothing was locked or unlocked";

/// Runs `body` with the login keychain locked, and puts it back as it was on every way out: a
/// return, an `Err` (a signal's `interrupted` among them) or a panic. It locks the keychain
/// unless it already was locked, and unlocks it again only when it locked it, so a keychain the
/// user locked beforehand stays locked. A state it cannot tell (`Unknown`: the probe timed out,
/// as it does while a SecurityAgent dialog sits unanswered) could be a lock the user made, so
/// it then changes nothing and runs nothing (`Locked::Unknown`). The unlock is a cleanup step,
/// no `Cmd`, so no signal cancels it. An unlock that fails is a harness error that says to
/// unlock by hand, ahead of whatever `body` met. `body` learns whether the keychain was locked
/// already.
pub fn with_login_locked<K: LoginKeychain, T>(
    keychain: &K,
    body: impl FnOnce(bool) -> Result<T, HarnessError>,
) -> Result<Locked<T>, HarnessError> {
    let was_locked = match keychain.state() {
        LockState::Unknown => return Ok(Locked::Unknown),
        LockState::Locked => true,
        LockState::Unlocked => false,
    };
    if !was_locked {
        keychain.lock()?;
    }
    let ran = catch_unwind(AssertUnwindSafe(|| body(was_locked)));
    let unlocked = if was_locked {
        Ok(())
    } else {
        keychain.unlock()
    };
    let by_hand = |e: HarnessError, also: String| {
        harness(format!(
            "{e}: your login keychain is still locked; unlock it by hand with `security unlock-keychain`{also}"
        ))
    };
    match (ran, unlocked) {
        (Ok(done), Ok(())) => done.map(Locked::Ran),
        (Ok(done), Err(e)) => Err(by_hand(
            e,
            done.err()
                .map_or_else(String::new, |met| format!(" (the check met: {met})")),
        )),
        (Err(panic), Ok(())) => resume_unwind(panic),
        (Err(_), Err(e)) => Err(by_hand(e, " (the check panicked)".to_owned())),
    }
}

/// §17 O3: on a locked login keychain in the GUI session, whether the lock check (doctor's,
/// bounded at 5 s) raises the unlock dialog, and whether the dialog outlives the killed
/// `security`; and that the existence probe still answers (Appendix A.3). It asks what you saw,
/// then unlocks the keychain again, which asks for your password. The keychain is put back as
/// the check found it however the check ends (`with_login_locked`).
pub fn locked_login_keychain(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let Some(go) = ask(
        "This locks your login keychain, asks what you saw, and then asks for your password to unlock it. Continue?",
    )
    .decided()?
    else {
        return Ok(Outcome::skip("needs a terminal to ask on"));
    };
    if !go {
        return Ok(Outcome::skip("declined"));
    }
    let login = Login(ctx.roots.home.join(LOGIN_KEYCHAIN));
    let was_locked =
        match with_login_locked(&login, |was_locked| observe_locked(ctx, &mut p, was_locked))? {
            Locked::Ran(was_locked) => was_locked,
            Locked::Unknown => return Ok(Outcome::skip(UNKNOWN_STATE)),
        };
    let state = login.state();
    let found = if was_locked {
        LockState::Locked
    } else {
        LockState::Unlocked
    };
    p.expect(
        "the login keychain is as the check found it",
        state == found,
        json!(format!("{state:?}")),
    );
    Ok(p.finish("the lock check and the probe answered on a locked login keychain"))
}

/// `locked-login-keychain`'s observations, made while the login keychain is locked.
fn observe_locked(ctx: &Ctx, p: &mut Probe, was_locked: bool) -> Result<bool, HarnessError> {
    p.note(
        "the login keychain was locked before the check",
        json!(was_locked),
    );
    eprintln!("cargo xtask compat: running tagteam doctor; leave any dialog alone until asked.");
    let t = Instant::now();
    let doctor = ctx
        .tagteam(&["doctor", "--json"])
        .timeout(Duration::from_secs(60))
        .run(&ctx.roots)?;
    let seconds = t.elapsed().as_secs_f64();
    p.expect(
        "doctor answered within its bound (§13.6)",
        seconds < 8.0 && doctor.code.is_some(),
        json!({"seconds": seconds, "exit": doctor.code}),
    );
    if let Some(v) = doctor.json() {
        let keychain: Vec<Value> = v["checks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["id"].as_str().is_some_and(|id| id.contains("keychain")))
            .cloned()
            .collect();
        p.note(
            "doctor's keychain checks",
            ctx.redact.value(&json!(keychain)),
        );
    }
    let dialog = ask("Did a dialog asking for your login keychain's password appear?")
        .decided()?
        .unwrap_or(false);
    p.note("a dialog appeared during doctor", json!(dialog));
    if dialog {
        let stayed = ask("Is that dialog still on screen now?")
            .decided()?
            .unwrap_or(false);
        p.note("the dialog outlived security's 5 s timeout", json!(stayed));
    }
    let item = ctx.item(&ctx.live(), ItemKind::OAuth)?;
    let t = Instant::now();
    let probe = item.exists();
    let probe_seconds = t.elapsed().as_secs_f64();
    p.expect(
        "the existence probe answers on the locked keychain (Appendix A.3)",
        probe.is_present() && probe_seconds < 5.0,
        json!({"read": read_name(&probe), "seconds": probe_seconds}),
    );
    let again = ask("Did a dialog appear just now, for the probe?")
        .decided()?
        .unwrap_or(false);
    p.expect("the probe raised no dialog", !again, json!(again));
    Ok(was_locked)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::compat::sys::interrupted;

    /// A login keychain double: its state, and the locks and unlocks it saw.
    struct Double {
        state: Cell<LockState>,
        locks: Cell<u32>,
        unlocks: Cell<u32>,
        /// `unlock` fails, leaving the keychain locked.
        unlock_fails: bool,
    }

    impl Double {
        fn new(state: LockState) -> Self {
            Self {
                state: Cell::new(state),
                locks: Cell::new(0),
                unlocks: Cell::new(0),
                unlock_fails: false,
            }
        }
    }

    impl LoginKeychain for Double {
        fn state(&self) -> LockState {
            self.state.get()
        }

        fn lock(&self) -> Result<(), HarnessError> {
            self.locks.set(self.locks.get() + 1);
            self.state.set(LockState::Locked);
            Ok(())
        }

        fn unlock(&self) -> Result<(), HarnessError> {
            self.unlocks.set(self.unlocks.get() + 1);
            if self.unlock_fails {
                return Err(harness("security unlock-keychain failed"));
            }
            self.state.set(LockState::Unlocked);
            Ok(())
        }
    }

    #[test]
    fn a_keychain_this_check_locked_is_unlocked_once_however_the_check_ends() {
        let login = Double::new(LockState::Unlocked);
        let stopped = with_login_locked(&login, |was_locked| -> Result<(), HarnessError> {
            assert!(!was_locked);
            assert_eq!(login.state(), LockState::Locked);
            // doctor's run, cut short by Ctrl-C.
            Err(interrupted(2))
        });
        assert_eq!(stopped.unwrap_err().0, "interrupted by SIGINT");
        assert_eq!((login.locks.get(), login.unlocks.get()), (1, 1));
        assert_eq!(login.state(), LockState::Unlocked);

        let login = Double::new(LockState::Unlocked);
        let panicked = catch_unwind(AssertUnwindSafe(|| {
            with_login_locked(&login, |_| -> Result<(), HarnessError> {
                panic!("a check bug")
            })
        }));
        assert!(panicked.is_err(), "the panic goes on to run_checks");
        assert_eq!(login.unlocks.get(), 1);
    }

    #[test]
    fn a_keychain_in_an_unknown_state_is_neither_locked_nor_unlocked() {
        // A dialog the user has not answered makes the probe time out (Appendix A.3): the
        // keychain may be locked by the user, so the check changes nothing and does not run.
        let login = Double::new(LockState::Unknown);
        let ran = std::cell::Cell::new(false);
        let refused = with_login_locked(&login, |_| -> Result<(), HarnessError> {
            ran.set(true);
            Ok(())
        });
        assert_eq!(refused, Ok(Locked::Unknown));
        assert!(!ran.get(), "the check's body never ran");
        assert_eq!((login.locks.get(), login.unlocks.get()), (0, 0));
        assert!(UNKNOWN_STATE.contains("answer or dismiss the dialog"));
    }

    #[test]
    fn a_keychain_locked_before_the_check_is_never_unlocked_by_it() {
        let login = Double::new(LockState::Locked);
        let seen = with_login_locked(&login, Ok).unwrap();
        assert_eq!(seen, Locked::Ran(true));
        let stopped = with_login_locked(&login, |_| -> Result<(), HarnessError> {
            Err(interrupted(15))
        });
        assert!(stopped.is_err());
        assert_eq!((login.locks.get(), login.unlocks.get()), (0, 0));
        assert_eq!(login.state(), LockState::Locked);
    }

    #[test]
    fn an_unlock_that_fails_says_the_keychain_is_still_locked_whatever_the_check_met() {
        let failing = || Double {
            unlock_fails: true,
            ..Double::new(LockState::Unlocked)
        };
        let by_hand = "your login keychain is still locked; unlock it by hand with `security unlock-keychain`";
        // The check finished; only the unlock failed.
        let login = failing();
        let e = with_login_locked(&login, |_| Ok(())).unwrap_err().0;
        assert!(
            e.starts_with("security unlock-keychain failed:") && e.contains(by_hand),
            "{e}"
        );
        assert_eq!((login.locks.get(), login.unlocks.get()), (1, 1));
        // The check met an error too: the unlock failure leads, and names what it met.
        let login = failing();
        let e = with_login_locked(&login, |_| -> Result<(), HarnessError> {
            Err(interrupted(2))
        })
        .unwrap_err()
        .0;
        assert!(
            e.contains(by_hand) && e.ends_with("(the check met: interrupted by SIGINT)"),
            "{e}"
        );
        // The check panicked: the panic is not resumed over a keychain left locked.
        let login = failing();
        let e = catch_unwind(AssertUnwindSafe(|| {
            with_login_locked(&login, |_| -> Result<(), HarnessError> {
                panic!("a check bug")
            })
        }))
        .expect("the unlock failure is returned, not the panic")
        .unwrap_err()
        .0;
        assert!(
            e.contains(by_hand) && e.ends_with("(the check panicked)"),
            "{e}"
        );
        // A keychain locked beforehand is never unlocked, so nothing can fail.
        let login = Double {
            unlock_fails: true,
            ..Double::new(LockState::Locked)
        };
        assert_eq!(with_login_locked(&login, Ok), Ok(Locked::Ran(true)));
        assert_eq!(login.unlocks.get(), 0);
    }
}
