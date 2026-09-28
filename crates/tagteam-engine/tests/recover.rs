mod common;

use common::{
    API_KEY, Fx, STRAY_API_KEY, assert_journal_cleared, crash_row, crashed_switch, dead_holder,
    journal, vault_fp, write_target_credential,
};
use serde_json::Value;
use tagteam_cc::ItemKind;
use tagteam_cc::live::Platform;
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::store::JournalRow;
#[cfg(feature = "test-hooks")]
use tagteam_engine::switch::SwitchReason;
use tagteam_provider::{Keychain, ProcessStamp, Provider};

fn any_mutation(fx: &Fx, id: &AccountId) {
    fx.engine.set_disabled(id, false).unwrap(); // takes the mutation lock, so it recovers
}

fn active(fx: &Fx) -> Option<AccountId> {
    fx.engine.store().unwrap().active(&fx.provider()).unwrap()
}

/// The oracle resolves every live token to `email`'s login.
fn oracle_says(fx: &Fx, email: &str) {
    let owner = fx.cc.parse_identity(&Fx::oauth_account(email)).unwrap();
    fx.oracle.set(Some(owner));
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
    assert_eq!(active(&fx), Some(a));
    assert_journal_cleared(&fx);
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
    assert_journal_cleared(&fx);
}

#[test]
fn a_rotated_target_the_oracle_attributes_finishes_forward() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.rotate_live("rt-a-rotated-by-cc");
    oracle_says(&fx, "a@x.co");
    any_mutation(&fx, &a);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-a-rotated-by-cc"),
        "never written back"
    );
    assert_journal_cleared(&fx);
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
    assert_journal_cleared(&fx);
    assert_eq!(active(&fx), Some(b), "kept");
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
    oracle_says(&fx, "b@x.co");
    any_mutation(&fx, &a);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-b-rotated-by-cc")
    );
    assert_journal_cleared(&fx);
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
    assert_journal_cleared(&fx);
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

#[test]
fn a_cross_axis_switch_never_displaces_the_journaled_outgoing_generation() {
    // OAuth → API key, killed after the key was stored and before the entry was cleared. The
    // entry still holds the outgoing account's journaled generation, which step 4 already
    // settled with its vault: clearing it loses nothing. A generation CC rotated since the
    // crash was never settled, so that one is saved first.
    for rotated in [false, true] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let k = fx.add_api_key(API_KEY); // leaves a live
        crashed_switch(&fx, &a, &k);
        fx.put_managed_key(API_KEY.as_bytes());
        if rotated {
            fx.rotate_live("rt-a-rotated-by-cc");
        }
        any_mutation(&fx, &a);
        assert!(journal(&fx).is_none(), "rotated={rotated}");
        assert_eq!(active(&fx), Some(k), "rotated={rotated}");
        assert_eq!(fx.live_email().as_deref(), Some("api-key-2@token.local"));
        assert_eq!(
            fx.live_refresh_token(),
            None,
            "only machine-shared keys remain"
        );
        let displaced = fx.displaced();
        assert_eq!(displaced.len(), usize::from(rotated), "rotated={rotated}");
        assert!(
            displaced
                .iter()
                .all(|d| String::from_utf8_lossy(d).contains("rt-a-rotated-by-cc"))
        );
    }
}

#[test]
fn the_reverse_cross_axis_switch_never_displaces_the_outgoing_key() {
    // API key → OAuth, killed after the entry was written and before the managed key was
    // cleared: the key left behind is the outgoing account's journaled generation.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    fx.switch_to(&k, false).unwrap();
    let displaced_before = fx.displaced().len();
    crashed_switch(&fx, &k, &a);
    write_target_credential(&fx, &a);
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert_eq!(active(&fx), Some(a));
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(fx.managed_key(), None);
    assert_eq!(fx.displaced().len(), displaced_before);
}

#[test]
fn an_api_key_outgoing_account_finishes_backward_on_its_own_axis() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    fx.switch_to(&k, false).unwrap();
    crashed_switch(&fx, &k, &a);
    // Nothing reached the auth axes, but the identity rollback failed.
    common::splice_oauth_account(&fx.paths().global_config, &Fx::oauth_account("a@x.co"));
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert_eq!(active(&fx), Some(k));
    assert_eq!(fx.live_email().as_deref(), Some("api-key-2@token.local"));
    assert_eq!(fx.managed_key().as_deref(), Some(API_KEY.as_bytes()));
}

#[test]
fn the_linux_file_store_recovers_both_ways() {
    let fx = Fx::with_platform(Platform::Linux);
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(active(&fx), Some(a.clone()));

    crashed_switch(&fx, &a, &b);
    common::splice_oauth_account(&fx.paths().global_config, &Fx::oauth_account("b@x.co"));
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(active(&fx), Some(a));
}

#[test]
fn live_bytes_beat_a_contradicting_oracle_both_ways() {
    for landed in [false, true] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        crashed_switch(&fx, &b, &a);
        if landed {
            write_target_credential(&fx, &a);
        }
        // The oracle names the account the live bytes do not.
        let (decided, contradicted) = if landed {
            (&a, "b@x.co")
        } else {
            (&b, "a@x.co")
        };
        oracle_says(&fx, contradicted);
        any_mutation(&fx, &a);
        assert!(journal(&fx).is_none(), "landed={landed}");
        assert_eq!(active(&fx).as_ref(), Some(decided), "landed={landed}");
        assert_ne!(fx.live_email().as_deref(), Some(contradicted));
    }
}

#[test]
fn an_oracle_answer_without_a_uuid_never_decides() {
    // §7.6: an answer with no non-empty uuid of its own attributes nothing.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.rotate_live("rt-a-rotated-by-cc");
    let mut owner = Fx::oauth_account("a@x.co");
    owner["accountUuid"] = Value::String(String::new());
    fx.oracle.set(Some(fx.cc.parse_identity(&owner).unwrap()));
    any_mutation(&fx, &a);
    assert!(journal(&fx).is_some());
}

#[test]
fn a_switch_after_a_decidable_row_recovers_it_then_proceeds() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.switch_to(&b, false).unwrap();
    assert_journal_cleared(&fx);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    assert_eq!(active(&fx), Some(b));
}

#[test]
fn a_forced_switch_plans_again_after_its_own_lock_recovers_a_row() {
    // The forced switch plans against b's identity, then its mutation lock finishes the row
    // forward to a: under the lock the live login has moved, so it plans again.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.switch_to(&b, true).unwrap();
    assert_journal_cleared(&fx);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    assert_eq!(active(&fx), Some(b));
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
    assert_ne!(active(&fx), Some(a));
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
    assert_journal_cleared(&fx);
}

/// Review Focus 3 for a double-fired bare `switch`, which the thread race above cannot pin: two
/// direct targets are idempotent, but a rotation that re-planned from where the other one
/// landed would move two accounts ahead. Driven through the `planned` point so the race is
/// deterministic: the other process lands a→b while this one waits for the mutation lock.
#[cfg(feature = "test-hooks")]
#[test]
fn a_double_fired_rotation_switches_once() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c");
    fx.switch_to(&a, false).unwrap(); // a live and active: a rotation plans b
    let other = fx.engine_with_env(fx.env.clone());
    let req = fx.rotation_request(false);
    let first = req.clone();
    fx.engine.on_point(
        "planned",
        Box::new(move || assert!(other.switch(first.clone()).unwrap().switched)),
    );
    let out = fx.engine.switch(req).unwrap();
    assert_eq!(
        (out.switched, out.reason),
        (false, SwitchReason::AlreadyActive)
    );
    assert_eq!(out.from.map(|r| r.id), Some(b.clone()));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    assert_eq!(active(&fx), Some(b));
    assert_journal_cleared(&fx);
}
