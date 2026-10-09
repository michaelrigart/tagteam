mod common;

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use common::{
    API_KEY, Fx, STRAY_API_KEY, assert_journal_cleared, crash_row, crashed_switch, credential,
    dead_holder, journal, prev_refresh_token, vault_fp, write_target_credential,
};
use serde_json::Value;
use tagteam_cc::ItemKind;
use tagteam_cc::live::Platform;
use tagteam_core::AccountId;
use tagteam_core::autoswitch::AutoState;
use tagteam_engine::EngineError;
use tagteam_engine::oracle::HttpOracle;
use tagteam_engine::store::{Activation, JournalRow};
#[cfg(feature = "test-hooks")]
use tagteam_engine::switch::SwitchReason;
use tagteam_provider::http::Method;
use tagteam_provider::{Keychain, ProcessStamp, Provider};

fn any_mutation(fx: &Fx, _id: &AccountId) {
    // An account-changing command's mutation lock: it recovers, asking the oracle before it
    // locks (§9.6, §7.6). Metadata commands recover from fingerprints alone (Task 7), which
    // `metadata_commands_recover_without_asking_the_oracle` in tests/oracle.rs covers.
    drop(fx.engine.mutation_guard().unwrap());
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

/// Task 7's ruling: recovery records no auto-switch state, even for a switch an engine began.
/// The journal row names neither the switch's source nor its trigger, and the departure
/// snapshot was the dead tick's view of usage, which recovery cannot rebuild.
#[test]
fn a_recovered_switch_records_no_auto_switch_state() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: b
    let store = fx.engine.store().unwrap();
    store.set_unhealthy_ticks(&fx.provider(), 2).unwrap();
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    any_mutation(&fx, &a);
    assert_eq!(active(&fx), Some(a));
    assert_eq!(
        store.autoswitch_state(&fx.provider()).unwrap(),
        AutoState {
            unhealthy_ticks: 2,
            ..AutoState::default()
        }
    );
    let last = store.events().unwrap().pop().unwrap();
    assert_eq!(
        (last.kind.as_str(), last.source.as_str()),
        ("switch-recovered", "cli")
    );
}

#[test]
fn forward_recovery_displaces_a_stray_secret_on_the_axis_it_clears() {
    // §9.4 step 7's rule, applied by recovery: a key written between the journal row
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
            Err(e @ EngineError::InterruptedSwitch(_)) if e.to_string().contains("--force")
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
fn a_recovery_that_cannot_take_cc_s_lock_names_it_instead_of_advising_force() {
    // After a crash CC's lock directories stay behind, fresh, for up to a minute; or CC may be
    // mid-refresh. Recovery then cannot take them, which is not an undecidable row: every
    // refusal names the lock and says to retry, and never points at `--force`.
    let fx = Fx::with_lock_timeout(Duration::from_millis(300));
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a); // decidable: it finishes forward once the lock is free
    fs::create_dir(fx.paths().refresh_lock).unwrap();
    let refusals = [
        fx.switch_to(&b, false).map(drop),
        fx.engine.add_live(fx.add_options()).map(drop),
        fx.engine.add_token(fx.add_token_options(API_KEY)).map(drop),
        fx.engine.remove(&b).map(drop),
    ];
    for refusal in refusals {
        let err = refusal.unwrap_err();
        let msg = err.to_string();
        assert_eq!(err.kind(), "interrupted-switch", "{msg}");
        assert!(msg.contains(".oauth_refresh.lock"), "{msg}");
        assert!(msg.contains("retry once Claude Code is idle"), "{msg}");
        assert!(!msg.contains("--force"), "{msg}");
    }
    assert!(journal(&fx).is_some());
    fs::remove_dir(fx.paths().refresh_lock).unwrap();
    any_mutation(&fx, &a); // the retry the refusal advises
    assert_journal_cleared(&fx);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}

#[test]
fn a_recovery_waits_out_cc_s_config_lock_before_taking_any_lock() {
    // §9.1: the pre-wait comes first for recovery too, holding neither the mutation lock nor
    // CC's credential locks.
    let fx = Fx::with_lock_budgets(Duration::from_millis(300), Duration::from_secs(5));
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fs::create_dir(fx.paths().config_lock).unwrap(); // left behind, fresh

    std::thread::scope(|s| {
        let recovering = s.spawn(|| any_mutation(&fx, &a));
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(100));
            assert!(!recovering.is_finished(), "the recovery did not wait");
            let free = fs::create_dir(fx.paths().refresh_lock).is_ok();
            if free {
                fs::remove_dir(fx.paths().refresh_lock).unwrap();
            }
            assert!(free, "the pre-wait holds CC's refresh lock");
            assert!(
                common::mutation_lock_free(&fx.env),
                "the pre-wait holds the mutation lock"
            );
        }
        fs::remove_dir(fx.paths().config_lock).unwrap();
        recovering.join().unwrap();
    });

    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_journal_cleared(&fx);
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
fn an_unreadable_identity_never_passes_for_an_absent_one() {
    // §9.6: a row journaled with no live identity expects none. An `oauthAccount` that cannot
    // be read is not that absence, so the row is undecidable and stays, and nothing is written.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // the outgoing credential is live
    let row = JournalRow {
        from_id: None,
        from_identity: None,
        ..crash_row(&fx, &b, &a)
    };
    fx.engine.store().unwrap().insert_journal(&row).unwrap();
    let config = fx.paths().global_config;
    // An unpaired surrogate: the file splices, but the value does not parse.
    let unreadable = common::CLAUDE_JSON.replacen(
        '{',
        r#"{"oauthAccount": {"emailAddress": "b\ud800@x.co"},"#,
        1,
    );
    fs::write(&config, &unreadable).unwrap();
    assert!(matches!(
        fx.cc.live_identity(&fx.env),
        tagteam_provider::Read::Unreadable(_)
    ));
    any_mutation(&fx, &a);
    assert_eq!(journal(&fx), Some(row));
    assert_eq!(fs::read_to_string(&config).unwrap(), unreadable);
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
fn forward_recovery_to_an_api_key_saves_a_shadowed_credentials_file_before_stripping_it() {
    // §9.4 step 7, applied by recovery: clearing the entry for an API key strips the
    // credentials file behind the Keychain item too, and a generation there that no vault
    // holds is saved first.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY); // leaves a live
    crashed_switch(&fx, &a, &k);
    fx.put_managed_key(API_KEY.as_bytes()); // died after storing the key, before the entry
    let shadowed = Fx::credential_json("a@x.co", "rt-a-shadowed")
        .to_string()
        .into_bytes();
    let file = fx.paths().credentials_file;
    fs::write(&file, &shadowed).unwrap();
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert_eq!(fx.displaced(), [shadowed]);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&file).unwrap()).unwrap(),
        serde_json::json!({"mcpOAuth": {"srv": {"token": "machine-shared"}}}),
        "then stripped, as clearing the entry does"
    );
}

#[test]
fn forward_recovery_never_displaces_a_stale_mirror_the_vault_holds() {
    // CC refreshed the Keychain after the crash; the file still mirrors the outgoing account's
    // stored generation, which its vault keeps. Stripping it loses nothing.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    crashed_switch(&fx, &a, &k);
    fx.put_managed_key(API_KEY.as_bytes());
    fs::write(fx.paths().credentials_file, fx.vault_bytes(&a).unwrap()).unwrap();
    fx.rotate_live("rt-a-rotated-by-cc");
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    let displaced = fx.displaced();
    assert_eq!(displaced.len(), 1, "only the rotated entry itself");
    assert!(String::from_utf8_lossy(&displaced[0]).contains("rt-a-rotated-by-cc"));
}

#[test]
fn forward_recovery_to_an_api_key_leaves_an_inert_former_fallback_item_alone() {
    // Appendix A.2 (2.1.286): stripping the credential entry strips the suffixed item only.
    let fx = Fx::with_explicit_default_config_dir();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    crashed_switch(&fx, &a, &k);
    fx.put_managed_key(API_KEY.as_bytes());
    let (oauth, managed) = fx.put_inert_items();
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert!(fx.displaced().is_empty());
    assert_eq!(fx.inert_items(), (Some(oauth), Some(managed)));
    assert_eq!(
        fx.live_credential(),
        Some(serde_json::json!({"mcpOAuth": {"srv": {"token": "machine-shared"}}})),
        "the suffixed item was stripped"
    );
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
        to_epoch: Some(store.account(&b).unwrap().unwrap().login_epoch),
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

/// OAuth `a` → API key `k`, killed after step 7 stored the key and before the credential entry
/// was cleared; then CC rotated a's token. Returns `(a, k)`.
fn crashed_cross_axis_switch_then_cc_rotated(fx: &Fx) -> (AccountId, AccountId) {
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY); // leaves a live
    crashed_switch(fx, &a, &k);
    fx.put_managed_key(API_KEY.as_bytes());
    fx.rotate_live("rt-a-rotated-by-cc");
    (a, k)
}

#[test]
fn forward_recovery_captures_a_rotated_outgoing_token_the_oracle_attributes() {
    // Ruling L476: the rotated generation is a's newest. Capturing it keeps a usable;
    // displacing it would leave a's vault holding the spent rt-a.
    let fx = Fx::new();
    let (a, k) = crashed_cross_axis_switch_then_cc_rotated(&fx);
    oracle_says(&fx, "a@x.co");
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert_eq!(active(&fx), Some(k));
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a-rotated-by-cc")
    );
    assert_eq!(
        prev_refresh_token(&fx, &a).as_deref(),
        Some("rt-a"),
        ".prev keeps the old one"
    );
    assert!(
        fx.displaced().is_empty(),
        "captured, so nothing to displace"
    );
    assert_eq!(
        fx.live_refresh_token(),
        None,
        "the entry is still cleared for the API key"
    );
}

#[test]
fn a_captured_rotation_backfills_a_missing_account_uuid() {
    let fx = Fx::new();
    let (a, _) = crashed_cross_axis_switch_then_cc_rotated(&fx);
    // No uuid recorded yet: attribution falls back to email and org (§7.6, oracle.rs).
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "UPDATE accounts SET account_uuid = NULL WHERE id = ?1",
            [a.as_str()],
        )
        .unwrap();
    oracle_says(&fx, "a@x.co");
    any_mutation(&fx, &a);
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a-rotated-by-cc")
    );
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert_eq!(row.account_uuid.as_deref(), Some("uuid-a@x.co"));
}

#[test]
fn forward_recovery_without_an_attribution_displaces_the_rotated_token() {
    // No answer, and an answer naming someone else: displaced, as before the amendment.
    // Step 4's Unresolved capture does not apply to recovery.
    for answer in [None, Some("stranger@x.co")] {
        let fx = Fx::new();
        let (a, _) = crashed_cross_axis_switch_then_cc_rotated(&fx);
        if let Some(email) = answer {
            oracle_says(&fx, email);
        }
        any_mutation(&fx, &a);
        assert_journal_cleared(&fx);
        assert_eq!(
            fx.vault_refresh_token(&a).as_deref(),
            Some("rt-a"),
            "{answer:?}"
        );
        let displaced = fx.displaced();
        assert_eq!(displaced.len(), 1, "{answer:?}");
        assert!(String::from_utf8_lossy(&displaced[0]).contains("rt-a-rotated-by-cc"));
    }
}

#[test]
fn an_attributed_token_without_a_refresh_token_never_replaces_a_complete_vault() {
    // §6.2: an automatic capture never replaces a refresh token with a credential that lacks
    // one. Such an entry is displaced instead.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    crashed_switch(&fx, &a, &k);
    fx.put_managed_key(API_KEY.as_bytes());
    let access_only = serde_json::json!({
        "claudeAiOauth": {"accessToken": "at-a-only", "expiresAt": 1_790_003_600_000i64},
        "mcpOAuth": {"srv": {"token": "machine-shared"}}
    })
    .to_string()
    .into_bytes();
    fx.set_live_credential(&access_only);
    oracle_says(&fx, "a@x.co");
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(fx.displaced(), [access_only]);
}

#[test]
fn a_metadata_command_recovers_by_fingerprint_without_the_network() {
    // The row is decidable by fingerprint: the managed key is the target's. A metadata command
    // settles it without asking the oracle (§7.6, §9.6), so the rotated token has no
    // attribution and is displaced. The same row under an account-changing command asks the
    // oracle over HTTP once, and captures it.
    for asks in [false, true] {
        let fx = Fx::new();
        let (a, _) = crashed_cross_axis_switch_then_cc_rotated(&fx);
        fx.script_profile("a@x.co");
        let engine =
            fx.engine_with_oracle(Arc::new(HttpOracle::new(fx.http.clone(), fx.clock.clone())));
        if asks {
            drop(engine.mutation_guard().unwrap());
        } else {
            engine.set_disabled(&a, false).unwrap();
        }
        assert_journal_cleared(&fx);
        let profile_requests = fx.http.count(Method::Get, &Fx::endpoints().profile);
        assert_eq!(profile_requests, usize::from(asks), "asks={asks}");
        if asks {
            assert_eq!(
                fx.vault_refresh_token(&a).as_deref(),
                Some("rt-a-rotated-by-cc")
            );
            assert!(fx.displaced().is_empty());
        } else {
            assert!(fx.http.requests().is_empty(), "no request of any kind");
            assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
            assert_eq!(fx.displaced().len(), 1);
        }
    }
}

#[test]
fn backward_recovery_keeps_a_cc_updated_oauth_account_of_the_same_identity() {
    // The switch never landed; since the crash, CC refreshed a field of b's own oauthAccount.
    // That object still names b, so recovery keeps it rather than splicing the journaled copy
    // back over it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    let mut updated = Fx::oauth_account("b@x.co");
    updated["displayName"] = Value::String("B".into());
    common::splice_oauth_account(&fx.paths().global_config, &updated);
    any_mutation(&fx, &a);
    assert_journal_cleared(&fx);
    assert_eq!(active(&fx), Some(b));
    let doc: Value = serde_json::from_slice(&fs::read(fx.paths().global_config).unwrap()).unwrap();
    assert_eq!(
        doc["oauthAccount"], updated,
        "the CC-updated object is kept"
    );
}

#[test]
fn forward_recovery_never_captures_a_replaced_live_login() {
    // §9.6, §12.5: a's login was replaced while Claude Code kept the old one; then a switch to
    // k died after writing the key, and Claude Code rotated its copy of the old lineage. The
    // oracle attributes that rotation to a, yet capturing it would undo the replacement: it
    // is displaced.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY); // leaves a live
    fx.replace_login(&a, &credential("a@x.co", "rt-a-new"), "oauth");
    crashed_switch(&fx, &a, &k);
    fx.put_managed_key(API_KEY.as_bytes());
    fx.rotate_live("rt-a-rotated-by-cc");
    oracle_says(&fx, "a@x.co");

    any_mutation(&fx, &a);

    assert_journal_cleared(&fx);
    assert_eq!(active(&fx), Some(k));
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a-new"),
        "the replacement stays"
    );
    let displaced = fx.displaced();
    assert_eq!(displaced.len(), 1);
    assert!(String::from_utf8_lossy(&displaced[0]).contains("rt-a-rotated-by-cc"));
}

#[test]
fn forward_recovery_records_the_row_s_epoch_so_a_later_replacement_stays_stale_marked() {
    // §9.6: a forward finish records the row's `to_epoch`. A replacement that landed on a
    // since the row was written then leaves the live store stale-marked. A row from before the
    // column falls back to a's current epoch.
    for (to_epoch, stale) in [(Some(0), true), (None, false)] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live: b
        let row = JournalRow {
            to_epoch,
            ..crash_row(&fx, &b, &a)
        };
        fx.engine.store().unwrap().insert_journal(&row).unwrap();
        write_target_credential(&fx, &a); // the switch landed a's credential, then died
        // Then a replacement of a's login wrote the vault and its replacer died; recovery's
        // own account lock finishes it before recovering the row.
        fx.begin_replacement_with(
            &a,
            &credential("a@x.co", "rt-a-new"),
            &Fx::oauth_account("a@x.co"),
            "oauth",
            false,
        );

        any_mutation(&fx, &a);

        assert_journal_cleared(&fx);
        let want = if stale { 0 } else { 1 };
        assert_eq!(
            fx.activation(),
            Some(Activation {
                account: a.clone(),
                epoch: Some(want)
            }),
            "to_epoch={to_epoch:?}"
        );
        assert_eq!(fx.live_store_stale(&a), stale, "to_epoch={to_epoch:?}");
    }
}
