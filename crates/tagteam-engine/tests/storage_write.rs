//! Claude Code's storage-write lock (§9.1) as a switch and a recovery meet it. Every write of a
//! CC credential entry waits for CC's own; under it the entry is read again. CC's dead-token
//! marking is written over; any other change to the account-scoped keys aborts the write, and
//! the switch rolls back; the machine-shared keys CC wrote meanwhile are kept. A rollback puts a
//! place back only while it holds exactly what the switch wrote there: once CC wrote to the
//! entry since, the rollback leaves it and keeps the row for recovery (§9.4 step 10, §9.6). The
//! waits inside a critical span are never cut short by a signal (§14.1).
#![cfg(feature = "test-hooks")]

mod common;

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use common::{
    API_KEY, Fx, INERT_ITEM, cc_holds_storage_write_from, cc_marks_dead, cc_marks_dead_and_more,
    cc_released, crashed_switch, dead_holder, journal, mutation_lock_free, write_target_credential,
    writer_holds_storage_write_from,
};
use serde_json::{Value, json};
use tagteam_cc::{ItemKind, keychain_account};
use tagteam_engine::EngineError;
use tagteam_provider::Keychain;

/// The command whose rollback failed exits: its journal row is now a dead holder's, so the next
/// command recovers it first (§9.6, §12.6).
fn the_command_exits(fx: &Fx) {
    let mut row = journal(fx).unwrap();
    row.holder = dead_holder();
    fx.engine.store().unwrap().insert_journal(&row).unwrap();
    fx.engine.fail_at(None);
}

/// Claude Code changing the live credential with `cc_write` once the switch has written its
/// target (the `after-credential` point), before anything fails.
fn cc_writes_after_the_credential(fx: &Fx, cc_write: fn(&mut Value)) {
    let (kc, (svc, acct)) = (fx.kc.clone(), fx.live_item(ItemKind::OAuth));
    fx.engine.on_point(
        "after-credential",
        Box::new(move || {
            let mut live: Value = serde_json::from_slice(&kc.get(&svc, &acct).unwrap()).unwrap();
            cc_write(&mut live);
            kc.put(&svc, &acct, live.to_string().as_bytes());
        }),
    );
}

#[test]
fn a_switch_writes_its_target_over_cc_s_dead_token_marking() {
    // §9.1: a marking is no conflict. Between the journal row and the credential write, CC
    // holds its lock, marks b's token dead and refreshes its MCP token; the switch goes ahead
    // over the marking and keeps CC's MCP token.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b
    let cc = cc_holds_storage_write_from(&fx, "after-journal", None, |live| {
        cc_marks_dead(live);
        live["mcpOAuth"] = json!({"srv": {"token": "refreshed-by-cc"}});
    });

    let out = fx.switch_to(&a, false);
    let ended = Instant::now();
    let released = cc_released(&cc);

    assert!(out.unwrap().switched);
    assert!(ended > released, "the switch waited for CC's lock");
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(
        fx.live_credential().unwrap()["mcpOAuth"],
        json!({"srv": {"token": "refreshed-by-cc"}})
    );
    assert!(journal(&fx).is_none());
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn a_switch_whose_entry_cc_changes_by_more_than_a_marking_rolls_back() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: b
    let config = fs::read(fx.paths().global_config).unwrap();
    // Between the journal row and the credential write, CC holds its lock and changes b's
    // account-scoped keys by more than a dead-token marking (§9.1).
    let cc = cc_holds_storage_write_from(&fx, "after-journal", None, cc_marks_dead_and_more);

    let out = fx.switch_to(&a, false);
    let ended = Instant::now();
    let released = cc_released(&cc);

    let err = out.unwrap_err();
    assert!(ended > released, "the switch waited for CC's lock");
    assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
    assert!(
        err.to_string().contains("changed by another writer"),
        "{err}"
    );
    let live = fx.live_credential().unwrap();
    assert_eq!(
        live["trustedDeviceToken"],
        json!("cc-device"),
        "CC's write stands"
    );
    assert_eq!(live["claudeAiOauth"]["refreshToken"], json!(""));
    assert_eq!(
        live["mcpOAuth"],
        json!({"srv": {"token": "machine-shared"}})
    );
    assert_eq!(fs::read(fx.paths().global_config).unwrap(), config);
    assert!(
        journal(&fx).is_none(),
        "nothing was written, so nothing is left to recover"
    );
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(b));
    assert!(!fx.paths().storage_write_lock.exists());
    assert!(mutation_lock_free(&fx.env));
}

#[test]
fn an_mcp_token_cc_refreshes_during_the_wait_is_kept_by_the_switch() {
    // B #61: the machine-shared keys are taken from the read under the lock.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let cc = cc_holds_storage_write_from(&fx, "after-journal", None, |live| {
        live["mcpOAuth"] = json!({"srv": {"token": "refreshed-by-cc"}});
    });

    let out = fx.switch_to(&a, false);
    let ended = Instant::now();
    let released = cc_released(&cc);

    assert!(out.unwrap().switched);
    assert!(ended > released, "the switch waited for CC's lock");
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(
        fx.live_credential().unwrap()["mcpOAuth"],
        json!({"srv": {"token": "refreshed-by-cc"}}),
        "CC's MCP write is never lost"
    );
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn a_rollback_leaves_a_target_cc_marked_since_and_keeps_the_row() {
    // A rollback puts a place back only while it holds exactly what the switch wrote there. CC
    // marked the target dead after the switch wrote it, by itself or with another change: the
    // rollback leaves CC's write, reports it, and the row stays (§9.4 step 10). A marked login
    // names no account, so the next command's recovery cannot decide the row and refuses,
    // pointing at `switch --force` (§9.6). CC's write is still there.
    for cc_write in [cc_marks_dead as fn(&mut Value), cc_marks_dead_and_more] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        cc_writes_after_the_credential(&fx, cc_write);
        fx.engine.fail_at(Some("after-credential"));

        let err = fx.switch_to(&a, false).unwrap_err();

        assert!(matches!(err, EngineError::RollbackFailed { .. }), "{err}");
        assert!(
            err.to_string().contains("changed since tagteam wrote it"),
            "{err}"
        );
        let live = fx.live_credential().unwrap();
        assert_eq!(
            live["claudeAiOauth"]["refreshToken"],
            json!(""),
            "CC's marking stands"
        );
        assert!(journal(&fx).is_some(), "the row stays for recovery");
        assert!(!fx.paths().storage_write_lock.exists());

        the_command_exits(&fx);
        let next = fx.switch_to(&b, false).unwrap_err();
        assert!(matches!(next, EngineError::InterruptedSwitch(_)), "{next}");
        assert!(next.to_string().contains("--force"), "{next}");
        assert_eq!(
            fx.live_credential().unwrap(),
            live,
            "CC's write is still there"
        );
    }
}

#[test]
fn on_linux_a_rollback_restores_the_file_untouched_and_leaves_one_cc_wrote_since() {
    // §9.1 on Linux: `.credentials.json` is the only place of the entry. Failing right after the
    // switch's write rolls the file back byte for byte while nothing else wrote it; once CC
    // wrote it since, the rollback leaves CC's write and keeps the row (§9.4 step 10).
    use tagteam_cc::live::Platform;
    let fx = Fx::with_platform(Platform::Linux);
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: b
    let file = fx.paths().credentials_file;
    let before = fs::read(&file).unwrap();
    fx.engine.fail_at(Some("after-credential"));

    let err = fx.switch_to(&a, false).unwrap_err();

    assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
    assert_eq!(fs::read(&file).unwrap(), before, "byte for byte");
    assert!(journal(&fx).is_none(), "a clean rollback leaves no row");
    assert!(!fx.paths().storage_write_lock.exists());
    assert_eq!(
        fx.engine.store().unwrap().active(&fx.provider()).unwrap(),
        Some(b)
    );

    let wiped = {
        let mut live: Value = serde_json::from_slice(&before).unwrap();
        cc_marks_dead(&mut live);
        live.to_string().into_bytes()
    };
    let (path, cc_write) = (file.clone(), wiped.clone());
    fx.engine.on_point(
        "after-credential",
        Box::new(move || fs::write(&path, &cc_write).unwrap()),
    );

    let err = fx.switch_to(&a, false).unwrap_err();

    assert!(matches!(err, EngineError::RollbackFailed { .. }), "{err}");
    assert!(
        err.to_string().contains("changed since tagteam wrote it"),
        "{err}"
    );
    assert_eq!(fs::read(&file).unwrap(), wiped, "CC's write stands");
    assert!(journal(&fx).is_some(), "the row stays for recovery");
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn ctrl_c_while_the_switch_write_waits_for_cc_still_commits() {
    // §14.1: steps 7–10 are a critical span, so the storage-write wait there is not a
    // cancellation point. The signal waits for the next one.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let cc = cc_holds_storage_write_from(
        &fx,
        "after-journal",
        Some(Duration::from_millis(100)),
        |_| {},
    );

    let out = fx.switch_to(&a, false);
    let ended = Instant::now();
    let released = cc_released(&cc);

    assert!(out.unwrap().switched, "the switch ran to its commit");
    assert!(ended > released, "the switch waited for CC's lock");
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(fx.engine.cancel().requested(), Some(libc::SIGINT));
    assert!(journal(&fx).is_none());
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn ctrl_c_while_a_recovery_write_waits_for_cc_lets_the_recovery_finish() {
    // §14.1: recovery's writes are a critical span too. The command stops at its next wait.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a); // died after writing the credential, before the identity
    let lock = fx.paths().storage_write_lock;
    fs::create_dir(&lock).unwrap(); // CC holds it as the command starts
    let cancel = fx.engine.cancel().clone();
    let cc = thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        cancel.request(libc::SIGINT);
        thread::sleep(Duration::from_millis(300));
        let at = Instant::now();
        fs::remove_dir(&lock).unwrap();
        at
    });

    let out = fx.switch_to(&b, false);
    let ended = Instant::now();
    let released = cc.join().unwrap();
    let err = out.unwrap_err();

    assert_eq!(err.signal(), Some(libc::SIGINT), "{err}");
    assert!(
        matches!(err, EngineError::Lock(_)),
        "stopped at its next lock wait, after recovery: {err:?}"
    );
    assert!(journal(&fx).is_none(), "recovery committed");
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(a));
    assert_eq!(
        store.events().unwrap().last().unwrap().kind,
        "switch-recovered"
    );
    assert!(ended > released, "recovery waited for CC's lock");
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn another_writer_changing_an_inert_item_never_rolls_a_switch_back() {
    // Appendix A.2 (2.1.286): the unsuffixed item of an explicit `~/.claude` is no place of the
    // entry, and §9.1 compares only places, so another writer's change to it is no conflict.
    let fx = Fx::with_explicit_default_config_dir();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b
    fx.put_inert_items();
    let cc = writer_holds_storage_write_from(&fx, "after-journal", None, INERT_ITEM, |item| {
        item["claudeAiOauth"]["refreshToken"] = json!("rt-other");
    });

    let out = fx.switch_to(&a, false);
    let ended = Instant::now();
    let released = cc_released(&cc);

    out.unwrap();
    assert!(ended > released, "the switch waited for the lock");
    let inert = fx.kc.get(INERT_ITEM, &keychain_account(&fx.env)).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&inert).unwrap()["claudeAiOauth"]["refreshToken"],
        json!("rt-other"),
        "the other writer's item stands"
    );
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert!(journal(&fx).is_none());
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn a_rollback_puts_each_place_back_byte_for_byte() {
    // Nothing wrote between the switch and its rollback, so every place goes back to exactly
    // what it held. The Keychain item and `.credentials.json` hold different MCP tokens; an
    // API-key switch clears the OAuth entry at both, keeping each one's, then fails.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a"); // live: a, in the Keychain
    let key = fx.add_api_key(API_KEY);
    let mut file = fx.live_credential().unwrap();
    file["mcpOAuth"] = json!({"srv": {"token": "the-file's-own"}});
    fs::write(fx.paths().credentials_file, file.to_string()).unwrap();
    let item_before = fx.live_credential();
    let file_before = fs::read(fx.paths().credentials_file).unwrap();
    fx.engine.fail_at(Some("after-credential"));

    let err = fx.switch_to(&key, false).unwrap_err();

    assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
    assert_eq!(fx.live_credential(), item_before);
    assert_eq!(
        fs::read(fx.paths().credentials_file).unwrap(),
        file_before,
        "the file keeps its own MCP token"
    );
    assert!(journal(&fx).is_none());
}

#[test]
fn a_rollback_keeps_the_mcp_token_cc_refreshed_while_the_switch_waited() {
    // §9.1: CC refreshes its MCP token while the switch waits for the storage-write lock; the
    // switch writes its target carrying the new token, then fails. The rollback puts b's login
    // back with the token CC refreshed, never the one the snapshot read before CC's write.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b
    let mut want = fx.live_credential().unwrap();
    want["mcpOAuth"] = json!({"srv": {"token": "refreshed-by-cc"}});
    let cc = cc_holds_storage_write_from(&fx, "after-journal", None, |live| {
        live["mcpOAuth"] = json!({"srv": {"token": "refreshed-by-cc"}});
    });
    fx.engine.fail_at(Some("after-credential"));

    let out = fx.switch_to(&a, false);
    let ended = Instant::now();
    let released = cc_released(&cc);

    let err = out.unwrap_err();
    assert!(ended > released, "the switch waited for CC's lock");
    assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
    assert_eq!(
        fx.live_credential().unwrap(),
        want,
        "b's login is back, with CC's refreshed MCP token"
    );
    assert!(journal(&fx).is_none());
    assert!(!fx.paths().storage_write_lock.exists());
}

#[test]
fn a_rolled_back_fallback_from_a_file_login_creates_no_keychain_item() {
    // Codex round 5: the live login is in `.credentials.json` with no Keychain item. The
    // target's Keychain write fails, so the write falls back to the file; the Keychain
    // recovers and the switch fails. The rollback must put the file login back, MCP keys and
    // all, and leave no Keychain item, which CC would read first.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b, in the Keychain
    let (svc, acct) = fx.live_item(ItemKind::OAuth);
    let login = fx.kc.get(&svc, &acct).unwrap();
    fs::write(fx.paths().credentials_file, &login).unwrap();
    tagteam_provider::Keychain::delete(&*fx.kc, &svc, &acct).unwrap(); // b's login is in the file only
    fx.kc.set_fail_write(&svc, true);
    let (kc, healed) = (fx.kc.clone(), svc.clone());
    fx.engine.on_point(
        "after-credential",
        Box::new(move || kc.set_fail_write(&healed, false)),
    );
    fx.engine.fail_at(Some("after-credential"));

    let err = fx.switch_to(&a, false).unwrap_err();

    assert!(matches!(err, EngineError::RolledBack(_)), "{err}");
    assert_eq!(
        fx.kc.get(&svc, &acct),
        None,
        "no Keychain item hides the file login"
    );
    assert_eq!(fs::read(fx.paths().credentials_file).unwrap(), login);
    assert!(journal(&fx).is_none());
}

#[test]
fn a_rollback_leaves_the_item_cc_refreshed_and_recovery_finishes_forward() {
    // Codex rounds 6 and 7: the live login is in `.credentials.json` with no Keychain item. The
    // switch creates the item and mirrors the file; CC then refreshes its MCP token in the
    // item, the only place that holds it, and the switch fails. The rollback leaves the entry
    // as it is and keeps the row. The next command finds a's login live and finishes the
    // switch forward, CC's token kept (§9.6).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b, in the Keychain
    let (svc, acct) = fx.live_item(ItemKind::OAuth);
    let login = fx.kc.get(&svc, &acct).unwrap();
    fs::write(fx.paths().credentials_file, &login).unwrap();
    fx.kc.delete(&svc, &acct).unwrap(); // b's login is in the file only
    cc_writes_after_the_credential(&fx, |live| {
        live["mcpOAuth"] = json!({"srv": {"token": "refreshed-by-cc"}});
    });
    fx.engine.fail_at(Some("after-credential"));

    let err = fx.switch_to(&a, false).unwrap_err();

    assert!(matches!(err, EngineError::RollbackFailed { .. }), "{err}");
    assert!(
        err.to_string()
            .contains(&format!("{svc} (changed since tagteam wrote it")),
        "{err}"
    );
    let item = fx.kc.get(&svc, &acct).unwrap();
    let cc: Value = serde_json::from_slice(&item).unwrap();
    assert_eq!(cc["claudeAiOauth"]["refreshToken"], json!("rt-a"));
    assert_eq!(
        cc["mcpOAuth"],
        json!({"srv": {"token": "refreshed-by-cc"}}),
        "CC's write stands"
    );
    assert!(journal(&fx).is_some(), "the row stays for recovery");

    the_command_exits(&fx);
    drop(fx.engine.mutation_guard().unwrap());

    assert!(journal(&fx).is_none(), "recovery finished the switch");
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(a));
    assert_eq!(
        fx.kc.get(&svc, &acct).unwrap(),
        item,
        "with CC's token kept"
    );
}

#[test]
fn a_recovery_stopped_by_cc_writing_the_entry_meanwhile_asks_for_a_plain_retry() {
    // §9.6: forward recovery to an API key clears the OAuth entry's account keys under CC's
    // storage-write lock. CC holds the lock and changes the entry by more than a dead-token
    // marking while recovery waits: the clear aborts (`EntryMoved`) and the row stays. A retry
    // settles it, so the refusal says that, never `switch --force`, which is for a row recovery
    // cannot decide.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY); // leaves a live
    crashed_switch(&fx, &a, &k);
    fx.put_managed_key(API_KEY.as_bytes()); // died after storing the key, before the entry
    let cc = cc_holds_storage_write_from(&fx, "before-mutation-lock", None, cc_marks_dead_and_more);

    let err = fx.engine.remove(&a).unwrap_err();
    cc_released(&cc);

    assert!(matches!(err, EngineError::RecoveryMoved { .. }), "{err:?}");
    assert_eq!(err.kind(), "interrupted-switch");
    let message = err.to_string();
    assert!(message.contains("retry"), "{message}");
    assert!(!message.contains("--force"), "{message}");
    assert!(journal(&fx).is_some(), "the row stays for the retry");
    assert!(!fx.paths().storage_write_lock.exists());
}
