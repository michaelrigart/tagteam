//! §7.4 and B.71: every quarantine that is cleared records exactly one `unquarantine` event,
//! whichever path clears it. Its reason is `account-replaced` when the account's `login_epoch`
//! moved, else `credentials-replaced` (§11.4), and its source is the command's.
mod common;

use common::{Fx, credential, due, vault_fp};
use serde_json::json;
use tagteam_core::AccountId;
use tagteam_engine::store::EventRow;

/// Every `unquarantine` event recorded so far, as (account, reason, source).
fn unquarantines(fx: &Fx) -> Vec<(AccountId, String, String)> {
    fx.engine
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "unquarantine")
        .map(|e: EventRow| {
            (
                e.to_id.expect("an unquarantine names its account"),
                e.detail.expect("an unquarantine carries its reason")["reason"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                e.source,
            )
        })
        .collect()
}

fn one(id: &AccountId, reason: &str, source: &str) -> Vec<(AccountId, String, String)> {
    vec![(id.clone(), reason.to_owned(), source.to_owned())]
}

/// Quarantines `id`, bound to its vault's generation, as a Dead verdict would (§7.4).
fn quarantine(fx: &Fx, id: &AccountId) {
    fx.quarantine(id, "invalid_grant", &vault_fp(fx, id));
}

#[test]
fn add_over_a_quarantined_account_records_account_replaced() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    quarantine(&fx, &a);
    fx.add("a@x.co", "rt-a2"); // a new login of the same account
    assert_eq!(unquarantines(&fx), one(&a, "account-replaced", "cli"));
}

#[test]
fn add_token_over_a_quarantined_token_account_records_account_replaced() {
    let fx = Fx::new();
    let token = |key: &str| {
        let mut o = fx.add_token_options(key);
        o.email = Some("keys@x.co".into());
        o
    };
    let k = fx
        .engine
        .add_token(token("sk-ant-api03-first-key"))
        .unwrap()
        .account
        .id;
    quarantine(&fx, &k);
    fx.engine
        .add_token(token("sk-ant-api03-second-key"))
        .unwrap();
    assert_eq!(unquarantines(&fx), one(&k, "account-replaced", "cli"));
}

#[test]
fn a_landed_replacement_reconciled_by_the_next_lock_holder_records_account_replaced() {
    // §12.5: the replacer died after its vault write; the next holder installs the login.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live
    quarantine(&fx, &a);
    fx.begin_replacement(
        &a,
        &credential("a@x.co", "rt-a9"),
        &Fx::oauth_account("a@x.co"),
        "oauth",
    );
    drop(fx.engine.lock_account(&a).unwrap());
    assert_eq!(unquarantines(&fx), one(&a, "account-replaced", "cli"));
}

#[test]
fn the_switch_s_outgoing_capture_records_credentials_replaced_with_its_source() {
    // M2a's m-9: the capture wrote the vault and cleared the quarantine with no event.
    for source in ["cli", "auto"] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live
        quarantine(&fx, &b);
        fx.rotate_live("rt-b2"); // CC refreshed b in place
        let mut req = fx.switch_request(&a, false);
        req.source = source;
        fx.engine.switch(req).unwrap();
        assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b2"));
        assert_eq!(
            unquarantines(&fx),
            one(&b, "credentials-replaced", source),
            "{source}"
        );
    }
}

#[test]
fn a_capture_of_an_account_with_no_quarantine_records_nothing() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live
    fx.rotate_live("rt-b2");
    fx.switch_to(&a, false).unwrap();
    fx.add("a@x.co", "rt-a2"); // a replacement of an account with no quarantine
    assert!(unquarantines(&fx).is_empty());
}

#[test]
fn a_refresh_after_the_gate_releases_a_stale_quarantine_records_one_event() {
    // §7.4: the gate releases a quarantine bound to an older generation, then its refresh
    // writes a new fingerprint through `persist_generation`, which finds nothing left to clear.
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a3"));
    fx.quarantine(&a, "invalid_grant", "sha256:an-older-generation");
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.engine
        .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
        .unwrap();
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a3"));
    assert_eq!(unquarantines(&fx), one(&a, "credentials-replaced", "cli"));
}

#[test]
fn the_tick_s_release_of_a_landed_replacement_records_account_replaced_once() {
    // A replacement that landed moved the epoch before its marker was cleared: releasing the
    // quarantine it left is the replacement's clear, and the reconciliation after it finds
    // nothing left to clear.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live
    quarantine(&fx, &a);
    fx.begin_replacement(
        &a,
        &credential("a@x.co", "rt-a9"),
        &Fx::oauth_account("a@x.co"),
        "oauth",
    );
    assert_eq!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "auto")
            .unwrap(),
        [a.clone()]
    );
    drop(fx.engine.lock_account(&a).unwrap());
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert_eq!(row.replacing_fp, None, "reconciled");
    assert_eq!(unquarantines(&fx), one(&a, "account-replaced", "auto"));
}

#[test]
fn the_tick_s_release_without_a_replacement_records_credentials_replaced() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    fx.engine
        .release_unbound_quarantines(&fx.provider(), "auto")
        .unwrap();
    assert_eq!(unquarantines(&fx), one(&a, "credentials-replaced", "auto"));
    let detail = fx
        .engine
        .store()
        .unwrap()
        .events()
        .unwrap()
        .pop()
        .unwrap()
        .detail;
    assert_eq!(detail, Some(json!({"reason": "credentials-replaced"})));
}
