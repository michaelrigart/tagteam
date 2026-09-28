mod common;

use common::{Fx, STRAY_API_KEY};
use serde_json::Value;
use tagteam_cc::ItemKind;
use tagteam_cc::shape::compose;
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::store::JournalRow;
use tagteam_provider::{Keychain, ProcessStamp, Provider};

/// A process that has exited: its journal rows are recoverable (§12.6).
fn dead_holder() -> ProcessStamp {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    ProcessStamp { pid, start: 0 }
}

fn vault_fp(fx: &Fx, id: &AccountId) -> String {
    fx.cc
        .fingerprint(&fx.vault_bytes(id).unwrap())
        .unwrap()
        .as_str()
        .to_owned()
}

/// The row a switch from `from` to `to` writes at step 6, held by a process that has died.
fn crash_row(fx: &Fx, from: &AccountId, to: &AccountId) -> JournalRow {
    let from_row = fx.engine.store().unwrap().account(from).unwrap().unwrap();
    JournalRow {
        provider: fx.provider(),
        holder: dead_holder(),
        from_id: Some(from.clone()),
        to_id: to.clone(),
        from_fp: Some(vault_fp(fx, from)),
        from_identity: Some(from_row.identity_json),
        to_fp: vault_fp(fx, to),
        started_at: 1,
        prior: None,
    }
}

/// Leaves a journal row as a switch from `from` to `to` that died after step 6.
fn crashed_switch(fx: &Fx, from: &AccountId, to: &AccountId) {
    let row = crash_row(fx, from, to);
    fx.engine.store().unwrap().insert_journal(&row).unwrap();
}

/// What step 7 leaves live: the target credential, composed with the live machine-shared keys.
fn write_target_credential(fx: &Fx, to: &AccountId) {
    let live: Value = fx.live_credential().unwrap();
    let composed = compose(&fx.vault_bytes(to).unwrap(), live.as_object()).unwrap();
    fx.set_live_credential(&composed);
}

fn any_mutation(fx: &Fx, id: &AccountId) {
    fx.engine.set_disabled(id, false).unwrap(); // takes the mutation lock, so it recovers
}

fn journal(fx: &Fx) -> Option<JournalRow> {
    fx.engine.store().unwrap().journal(&fx.provider()).unwrap()
}

#[test]
fn a_landed_credential_finishes_forward() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: b
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a); // died after writing the credential, before the identity
    any_mutation(&fx, &a);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(a));
    assert!(journal(&fx).is_none());
}

#[test]
fn forward_recovery_displaces_a_stray_secret_on_the_axis_it_clears() {
    // §9.4 step 7's off-axis rule, applied by recovery: a key written between the journal row
    // and the crash is saved before the managed-key axis is cleared, never just lost.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.put_managed_key(STRAY_API_KEY.as_bytes());
    any_mutation(&fx, &a);
    assert_eq!(fx.displaced(), [STRAY_API_KEY.as_bytes()]);
    assert_eq!(fx.managed_key(), None);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert!(journal(&fx).is_none());
}

#[test]
fn a_rotated_target_the_oracle_attributes_finishes_forward() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.rotate_live("rt-a-rotated-by-cc");
    let owner = fx.cc.parse_identity(&Fx::oauth_account("a@x.co")).unwrap();
    fx.oracle.set(Some(owner));
    any_mutation(&fx, &a);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-a-rotated-by-cc"),
        "never written back"
    );
    assert!(journal(&fx).is_none());
}

#[test]
fn an_unlanded_credential_finishes_backward_without_touching_it() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    // The credential rollback succeeded but the identity rollback did not.
    common::splice_oauth_account(&fx.paths().global_config, &Fx::oauth_account("a@x.co"));
    any_mutation(&fx, &a);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    assert!(journal(&fx).is_none());
    let store = fx.engine.store().unwrap();
    assert_eq!(store.active(&fx.provider()).unwrap(), Some(b), "kept");
}

#[test]
fn a_rotated_outgoing_credential_the_oracle_attributes_finishes_backward() {
    // The outgoing generation CC rotated after the crash is the one established as the
    // outgoing account's: the row settles on it, not on the journaled `from_fp`.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    common::splice_oauth_account(&fx.paths().global_config, &Fx::oauth_account("a@x.co"));
    fx.rotate_live("rt-b-rotated-by-cc");
    let owner = fx.cc.parse_identity(&Fx::oauth_account("b@x.co")).unwrap();
    fx.oracle.set(Some(owner));
    any_mutation(&fx, &a);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-b-rotated-by-cc")
    );
    assert!(journal(&fx).is_none());
}

#[test]
fn a_rotation_or_logout_after_the_crash_is_undecidable() {
    for logout in [false, true] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        crashed_switch(&fx, &b, &a);
        write_target_credential(&fx, &a);
        if logout {
            let (svc, acct) = fx.live_item(ItemKind::OAuth);
            fx.kc.delete(&svc, &acct).unwrap();
        } else {
            fx.rotate_live("rt-a-rotated-by-cc");
        }
        any_mutation(&fx, &a);
        assert!(journal(&fx).is_some(), "logout={logout}");
        assert!(matches!(
            fx.switch_to(&b, false),
            Err(EngineError::InterruptedSwitch(_))
        ));
        assert!(matches!(
            fx.engine.add_live(fx.add_options()),
            Err(EngineError::InterruptedSwitch(_))
        ));
        fx.switch_to(&b, true).unwrap();
        assert!(journal(&fx).is_none(), "logout={logout}");
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    }
}

#[test]
fn a_stale_file_behind_an_unreadable_keychain_is_undecidable() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a); // the Keychain now holds a's credential…
    let (svc, acct) = fx.live_item(ItemKind::OAuth);
    fx.kc.set_unreadable(&svc, &acct, true); // …but cannot be read,
    std::fs::write(
        fx.paths().credentials_file,
        Fx::credential_json("b@x.co", "rt-b").to_string(),
    )
    .unwrap(); // and a stale file says b
    any_mutation(&fx, &a);
    assert!(journal(&fx).is_some(), "never decided from a degraded read");
    fx.kc.set_unreadable(&svc, &acct, false);
    any_mutation(&fx, &a);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"), "once readable");
    assert!(journal(&fx).is_none());
}

#[test]
fn a_conflicting_auth_axis_keeps_the_row() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a); // b's credential and identity are still live…
    fx.put_managed_key(b"sk-ant-api03-c");
    any_mutation(&fx, &a); // …but an unrelated API key now authenticates too
    assert!(journal(&fx).is_some());
    assert_eq!(fx.managed_key().as_deref(), Some(&b"sk-ant-api03-c"[..]));
}

#[cfg(feature = "test-hooks")]
#[test]
fn a_row_that_appears_while_waiting_for_the_lock_is_recovered() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    write_target_credential(&fx, &a); // a switch to a had written the credential, then died
    let row = crash_row(&fx, &b, &a);
    // The row is written by "another process" after this command's pre-lock scan found nothing.
    let store = fx.engine.store().unwrap();
    let (writer, pending) = (store.clone(), std::sync::Mutex::new(Some(row)));
    fx.engine.on_point(
        "before-mutation-lock",
        Box::new(move || {
            if let Some(row) = pending.lock().unwrap().take() {
                writer.insert_journal(&row).unwrap();
            }
        }),
    );
    any_mutation(&fx, &a);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert!(store.journal(&fx.provider()).unwrap().is_none());
}

#[cfg(feature = "test-hooks")]
#[test]
fn forward_recovery_keeps_the_row_if_the_credential_changes_before_commit() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    // Between writing the identity and the final re-read, the credential becomes someone else's.
    let kc = fx.kc.clone();
    let (svc, acct) = fx.live_item(ItemKind::OAuth);
    fx.engine.on_point(
        "recovery-before-commit",
        Box::new(move || {
            kc.put(
                &svc,
                &acct,
                Fx::credential_json("c@x.co", "rt-c").to_string().as_bytes(),
            );
        }),
    );
    any_mutation(&fx, &a);
    assert!(journal(&fx).is_some());
    assert_ne!(
        fx.engine.store().unwrap().active(&fx.provider()).unwrap(),
        Some(a)
    );
}

#[cfg(feature = "test-hooks")]
#[test]
fn a_forced_switch_that_fails_puts_back_the_row_it_superseded() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.rotate_live("rt-a-rotated-by-cc"); // undecidable
    any_mutation(&fx, &a);
    let unresolved = journal(&fx).unwrap();
    fx.engine.fail_at(Some("after-journal"));
    assert!(matches!(
        fx.switch_to(&b, true),
        Err(EngineError::RolledBack(_))
    ));
    assert_eq!(journal(&fx), Some(unresolved), "the unresolved row is back");
}

#[test]
fn a_forced_switch_killed_before_landing_leaves_the_superseded_row() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.rotate_live("rt-a-rotated-by-cc");
    let store = fx.engine.store().unwrap();
    let unresolved = journal(&fx).unwrap();
    // `switch b --force` published its own row, carrying the old one, then died.
    let live_fp = fx
        .cc
        .fingerprint(fx.live_credential().unwrap().to_string().as_bytes())
        .unwrap()
        .as_str()
        .to_owned();
    let forced = JournalRow {
        provider: fx.provider(),
        holder: dead_holder(),
        from_id: Some(b.clone()),
        to_id: b.clone(),
        from_fp: Some(live_fp),
        from_identity: Some(store.account(&b).unwrap().unwrap().identity_json),
        to_fp: vault_fp(&fx, &b),
        started_at: 2,
        prior: Some(Box::new(unresolved.clone())),
    };
    store.insert_journal(&forced).unwrap();
    any_mutation(&fx, &a);
    assert_eq!(journal(&fx), Some(unresolved));
}

#[test]
fn a_live_holder_is_left_alone() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let mut row = crash_row(&fx, &b, &a);
    row.holder = ProcessStamp::current().unwrap();
    fx.engine.store().unwrap().insert_journal(&row).unwrap();
    write_target_credential(&fx, &a);
    any_mutation(&fx, &a);
    assert_eq!(
        fx.live_email().as_deref(),
        Some("b@x.co"),
        "not recovered while its holder lives"
    );
    assert_eq!(journal(&fx), Some(row));
}

#[test]
fn two_engines_never_double_switch() {
    // Review Focus 3.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let other = fx.engine_with_env(fx.env.clone());
    std::thread::scope(|s| {
        let t1 = s.spawn(|| fx.switch_to(&a, false));
        let t2 = s.spawn(|| other.switch(fx.switch_request(&b, false)));
        t1.join().unwrap().unwrap();
        t2.join().unwrap().unwrap();
    });
    // Whatever order they ran in, the live identity and credential name the same account.
    let email = fx.live_email().unwrap();
    let rt = fx.live_refresh_token().unwrap();
    assert!(
        (email == "a@x.co" && rt == "rt-a") || (email == "b@x.co" && rt == "rt-b"),
        "{email} / {rt}"
    );
    assert!(journal(&fx).is_none());
}
