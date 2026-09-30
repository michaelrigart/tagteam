//! §6.2 "Pending rescues before activation": a switch settles its target's `rescue/` entries
//! under the target's account lock before it reads the target, in both branches of §9.4.

mod common;

use std::fs;
use std::thread;
use std::time::Duration;

use common::{Fx, credential, journal, two_accounts, vault_fp};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::Provider;

#[test]
fn a_rescued_successor_is_adopted_and_is_what_claude_code_receives() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let rescue = fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    fx.switch_to(&a, false).unwrap();
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-a-2"),
        "never the spent rt-a"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    let prev: serde_json::Value =
        serde_json::from_slice(&fx.kc.get(SERVICE, &format!("{a}.prev")).unwrap()).unwrap();
    assert_eq!(
        prev["claudeAiOauth"]["refreshToken"], "rt-a",
        ".prev keeps the old generation"
    );
    assert!(
        !rescue.exists(),
        "deleted once the vault write was verified"
    );
}

#[test]
fn a_rescue_written_while_the_switch_waits_for_the_account_lock_is_adopted() {
    // The cross-review's Busy path. Another process's gate holds the target's account lock
    // while it refreshes; its vault write fails, so it rescues the successor, then releases
    // the lock. The switch that was waiting for that lock must activate the successor, not
    // the vault's generation, which that refresh has spent.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let held = AccountLock::acquire(&fx.env, &a, Duration::from_secs(1)).unwrap();
    let predecessor = vault_fp(&fx, &a);
    let successor = credential("a@x.co", "rt-a-2");
    let (fx_ref, a_ref) = (&fx, &a);
    thread::scope(|s| {
        s.spawn(move || {
            thread::sleep(Duration::from_millis(300));
            fx_ref.plant_rescue(a_ref, &predecessor, &successor);
            drop(held);
        });
        fx.switch_to(&a, false).unwrap();
    });
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
}

#[test]
fn a_damaged_rescue_file_refuses_the_switch_and_names_the_file() {
    // Review Focus 4: a truncated file, and a hand-edited one missing its credential.
    let damaged: [&[u8]; 2] = [
        br#"{"format":"tagteam-rescue","vers"#,
        br#"{"format":"tagteam-rescue","version":1}"#,
    ];
    for bytes in damaged {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let path = fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
        fs::write(&path, bytes).unwrap();
        let err = fx.switch_to(&a, false).unwrap_err();
        assert_eq!(err.kind(), "rescue-pending", "{err}");
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(err.to_string().contains(name), "names the file: {err}");
        assert_eq!(
            fx.live_email().as_deref(),
            Some("b@x.co"),
            "nothing activated"
        );
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
        assert!(journal(&fx).is_none(), "refused before the journal row");
        assert!(path.exists(), "a damaged rescue is never deleted");
    }
}

#[test]
fn a_rescue_whose_adoption_fails_refuses_the_switch() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let path = fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    fx.kc.set_fail_write(SERVICE, true);
    let err = fx.switch_to(&a, false).unwrap_err();
    fx.kc.set_fail_write(SERVICE, false);
    assert_eq!(err.kind(), "rescue-pending", "{err}");
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    assert!(journal(&fx).is_none());
    assert!(path.exists(), "kept until a verified vault write");
    // Once the vault accepts writes again, the same switch adopts and activates it.
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
    assert!(!path.exists());
}

#[test]
fn a_superseded_rescue_does_not_block_the_switch() {
    // Its predecessor is not the vault's generation: the vault moved on (a capture of a newer
    // lineage), so this rescue is superseded. It neither blocks nor gets activated.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let older = fx.cc.fingerprint(&credential("a@x.co", "rt-a-0")).unwrap();
    let path = fx.plant_rescue(&a, older.as_str(), &credential("a@x.co", "rt-a-stale"));
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert!(path.exists(), "left alone, never activated");
}

#[test]
fn the_direct_branch_settles_the_target_s_rescues_too() {
    // §9.4 step 2: an unmanaged live login, forced, takes the direct branch.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("stranger@x.co", "rt-s");
    fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    fx.switch_to(&a, true).unwrap();
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
}
