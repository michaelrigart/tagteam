//! §14.1: every lock wait is a cancellation point. A token set while a switch waits ends it
//! within one poll, with nothing written and no lock directory of tagteam's left behind
//! (Review Focus 1). Work done under the locks is never cut short: only waits check the token.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use common::{Fx, crashed_switch, journal, mutation_lock_free, write_target_credential};
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::store::EventRow;
use tagteam_provider::{Cancel, MutationGuard};

const SIGINT: i32 = libc::SIGINT;

/// Everything a switch could write: every Keychain item (the live credential, the managed key
/// and the vault), the global config, and the store's journal, active account and events.
#[derive(Debug, PartialEq)]
struct Written {
    items: BTreeMap<(String, String), Vec<u8>>,
    config: Vec<u8>,
    journal: bool,
    active: Option<AccountId>,
    events: Vec<EventRow>,
}

fn written(fx: &Fx) -> Written {
    let store = fx.engine.store().unwrap();
    Written {
        items: fx.kc.items(),
        config: fs::read(fx.paths().global_config).unwrap(),
        journal: journal(fx).is_some(),
        active: store.active(&fx.provider()).unwrap(),
        events: store.events().unwrap(),
    }
}

/// The three CC lock directories (§9.1).
fn cc_locks(fx: &Fx) -> [PathBuf; 3] {
    let p = fx.paths();
    [
        p.refresh_lock.clone(),
        p.legacy_lock(),
        p.config_lock.clone(),
    ]
}

/// Sets `cancel` to SIGINT from another thread once `after` has passed, as the CLI's signal
/// handler would (§14.1), and returns the instant just before it did.
fn interrupt_after(cancel: &Cancel, after: Duration) -> thread::JoinHandle<Instant> {
    let cancel = cancel.clone();
    thread::spawn(move || {
        thread::sleep(after);
        let at = Instant::now();
        cancel.request(SIGINT);
        at
    })
}

/// Runs `work` while a Ctrl-C arrives 200 ms in. `work` must still be waiting then and must
/// end with an error; returns the error and how long `work` ran on after the token was set.
fn interrupted<T: std::fmt::Debug>(
    fx: &Fx,
    work: impl FnOnce() -> Result<T, EngineError>,
) -> (EngineError, Duration) {
    let setter = interrupt_after(fx.engine.cancel(), Duration::from_millis(200));
    let result = work();
    let ended = Instant::now();
    let set_at = setter.join().unwrap();
    assert!(
        ended >= set_at,
        "it ended before the token was set, so it never waited: {result:?}"
    );
    (result.unwrap_err(), ended - set_at)
}

/// The interruption as the CLI reports it (Task 6): the signal and the `interrupted` kind,
/// within one poll of the token.
fn assert_interrupted(err: &EngineError, ran_on: Duration) {
    assert_eq!(err.signal(), Some(SIGINT), "{err}");
    assert_eq!(err.kind(), "interrupted", "{err}");
    // One poll: 100 ms for a flock, at most 500 ms for a CC lock. Every wait here would
    // otherwise run 9 s (CC), 10 s (the mutation lock) or 15 s (an account lock).
    assert!(
        ran_on < Duration::from_secs(1),
        "{ran_on:?} after the token"
    );
}

/// Review Focus 1: Claude Code holds `held` (it is refreshing), and the user presses Ctrl-C
/// while `switch` waits for it. CC's real 9 s budget is in force, so only the token can end the
/// wait this soon.
fn interrupted_while_cc_holds(held: fn(&Fx) -> PathBuf) {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.rotate_live("rt-b2"); // a capture made before the wait would show in the vault
    let held = held(&fx);
    fs::create_dir(&held).unwrap();
    let before = written(&fx);

    let (err, ran_on) = interrupted(&fx, || fx.switch_to(&a, false));

    assert_interrupted(&err, ran_on);
    assert!(
        err.to_string().contains(&held.display().to_string()),
        "names the lock it waited for: {err}"
    );
    assert_eq!(written(&fx), before, "nothing is written");
    for lock in cc_locks(&fx) {
        if lock == held {
            assert!(lock.is_dir(), "CC's lock is left alone");
        } else {
            assert!(!lock.exists(), "{} was left behind", lock.display());
        }
    }
    assert!(mutation_lock_free(&fx.env), "the mutation lock is released");
}

#[test]
fn ctrl_c_while_cc_holds_its_refresh_lock_ends_the_switch() {
    interrupted_while_cc_holds(|fx| fx.paths().refresh_lock);
}

#[test]
fn ctrl_c_while_cc_holds_its_legacy_lock_ends_the_switch() {
    interrupted_while_cc_holds(|fx| fx.paths().legacy_lock());
}

#[test]
fn ctrl_c_while_the_config_lock_is_held_ends_the_switch_in_the_pre_wait() {
    // §9.1: the config lock is waited out before any lock of tagteam's or CC's is taken, so
    // the interruption lands there and nothing of either kind exists to release.
    interrupted_while_cc_holds(|fx| fx.paths().config_lock);
}

#[test]
#[cfg(feature = "test-hooks")]
fn ctrl_c_while_cc_takes_the_config_lock_after_the_pre_wait_releases_the_credential_locks() {
    // CC takes its config lock once the pre-wait is over, so the switch holds the credential
    // locks when it waits for it: the interruption must release them.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let held = fx.paths().config_lock;
    let taken = held.clone();
    fx.engine.on_point(
        "config-pre-wait-done",
        Box::new(move || fs::create_dir(&taken).unwrap()),
    );
    let before = written(&fx);

    let (err, ran_on) = interrupted(&fx, || fx.switch_to(&a, false));

    assert_interrupted(&err, ran_on);
    assert!(
        err.to_string().contains(&held.display().to_string()),
        "names the lock it waited for: {err}"
    );
    assert_eq!(written(&fx), before, "nothing is written");
    for lock in cc_locks(&fx) {
        if lock == held {
            assert!(lock.is_dir(), "CC's lock is left alone");
        } else {
            assert!(!lock.exists(), "{} was left behind", lock.display());
        }
    }
    assert!(mutation_lock_free(&fx.env), "the mutation lock is released");
}

#[test]
fn ctrl_c_while_another_process_holds_an_account_lock_ends_the_switch() {
    // Another tagteam process's gate or vault write holds an account's lock: the target's,
    // which freshening waits for before the mutation lock (§9.2's lazy capture), or the
    // outgoing account's, which only `lock_accounts` takes, under the mutation lock.
    for target_held in [true, false] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        let before = written(&fx);
        let id = if target_held { &a } else { &b };
        let held = AccountLock::acquire(&fx.env, id, Duration::from_secs(1)).unwrap();

        let (err, ran_on) = interrupted(&fx, || fx.switch_to(&a, false));
        drop(held);

        assert_interrupted(&err, ran_on);
        assert!(
            matches!(err, EngineError::Lock(_)),
            "{target_held}: {err:?}"
        );
        assert_eq!(written(&fx), before, "nothing is written");
        assert!(
            cc_locks(&fx).iter().all(|l| !l.exists()),
            "CC's locks come after the account locks: none was taken"
        );
        assert!(mutation_lock_free(&fx.env), "the mutation lock is released");
    }
}

#[test]
fn ctrl_c_while_another_command_holds_the_mutation_lock_ends_the_switch() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let before = written(&fx);
    let held = MutationGuard::acquire(&fx.env, Duration::ZERO).unwrap(); // another command

    let (err, ran_on) = interrupted(&fx, || fx.switch_to(&a, false));
    drop(held);

    assert_interrupted(&err, ran_on);
    assert!(matches!(err, EngineError::Lock(_)), "{err:?}");
    assert_eq!(written(&fx), before, "nothing is written");
    assert!(cc_locks(&fx).iter().all(|l| !l.exists()));
    assert!(mutation_lock_free(&fx.env), "the mutation lock is released");
}

/// A recovery's lock waits are cancellation points too (§14.1). Interrupted there, it has
/// written nothing, its row stays for the next command, and the command reports the
/// interruption: never `interrupted-switch`, which would send the user to `--force`.
#[test]
fn ctrl_c_while_recovery_waits_for_cc_reports_the_interruption() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a); // died after writing the credential, before the identity
    fs::create_dir(fx.paths().refresh_lock).unwrap(); // CC is refreshing
    let before = written(&fx);

    let (err, ran_on) = interrupted(&fx, || fx.switch_to(&b, false));

    assert_interrupted(&err, ran_on);
    assert!(journal(&fx).is_some(), "the row waits for the next command");
    assert_eq!(written(&fx), before, "recovery wrote nothing");
    let [refresh, legacy, config] = cc_locks(&fx);
    assert!(refresh.is_dir(), "CC's lock is left alone");
    assert!(!legacy.exists() && !config.exists());
    assert!(mutation_lock_free(&fx.env), "the mutation lock is released");
}

/// §14.1: recovery's writes are a critical span. A token set while they run lets them finish
/// and commit; the command stops at its next lock wait, before it changes anything else.
#[cfg(feature = "test-hooks")]
#[test]
fn a_token_set_during_recovery_s_writes_lets_recovery_finish() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    let cancel = fx.engine.cancel().clone();
    fx.engine.on_point(
        "recovery-before-commit",
        Box::new(move || cancel.request(SIGINT)),
    );

    let err = fx.switch_to(&b, false).unwrap_err();

    assert_eq!(err.signal(), Some(SIGINT), "{err}");
    assert!(
        matches!(err, EngineError::Lock(_)),
        "stopped at its next lock wait: {err:?}"
    );
    assert!(journal(&fx).is_none(), "recovery committed");
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(a));
    assert_eq!(
        store.events().unwrap().last().unwrap().kind,
        "switch-recovered"
    );
    assert_eq!(
        fx.live_email().as_deref(),
        Some("a@x.co"),
        "b was never activated"
    );
    assert!(cc_locks(&fx).iter().all(|l| !l.exists()));
    assert!(mutation_lock_free(&fx.env), "the mutation lock is released");
}
