mod common;

use std::fs;
use std::time::Duration;

use common::{Fx, crashed_switch, credential, due, quarantine_of, token_requests, vault_fp};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::quarantine::QuarantineReason;
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::http::{HttpError, Method};
use tagteam_provider::{Clock, Keychain};

/// The gate on `id`, with the vault's current bytes as the caller's snapshot.
fn gate(fx: &Fx, id: &AccountId) -> GateOutcome {
    let snapshot = fx.vault_bytes(id).unwrap();
    fx.engine
        .refresh_stored(fx.cc.as_ref(), id, &snapshot)
        .unwrap()
}

#[test]
fn a_due_account_is_refreshed_once_and_its_successor_stored() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let GateOutcome::Refreshed(bytes) = gate(&fx, &a) else {
        panic!("expected Refreshed")
    };
    assert_eq!(fx.vault_bytes(&a).unwrap(), bytes);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    let prev: Value =
        serde_json::from_slice(&fx.kc.get(SERVICE, &format!("{a}.prev")).unwrap()).unwrap();
    assert_eq!(
        prev["claudeAiOauth"]["refreshToken"], "rt-a",
        "the old generation is .prev"
    );
    assert_eq!(token_requests(&fx), 1);
    let sent: Value =
        serde_json::from_slice(fx.http.requests().last().unwrap().body.as_deref().unwrap())
            .unwrap();
    assert_eq!(sent["grant_type"], "refresh_token");
    assert_eq!(sent["refresh_token"], "rt-a");
    assert_eq!(quarantine_of(&fx, &a), (None, None));
}

#[test]
fn another_holder_of_the_account_lock_makes_it_busy() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let _held = AccountLock::acquire(&fx.env, &a, Duration::ZERO).unwrap();
    assert!(matches!(gate(&fx, &a), GateOutcome::Busy));
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn the_live_login_is_owned_and_never_sent() {
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a"); // live: a
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a2"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Owned(OwnedBy::Live)));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn an_unreadable_live_identity_is_treated_as_live() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    fs::write(fx.paths().global_config, "{\n  \"oauthAccount\": ").unwrap(); // torn
    assert!(matches!(gate(&fx, &a), GateOutcome::Owned(OwnedBy::Live)));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn an_account_named_in_a_journal_row_is_owned() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a2"));
    crashed_switch(&fx, &b, &a);
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Owned(OwnedBy::Journal)
    ));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_quarantined_account_is_never_sent() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::InvalidGrant)
    ));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_quarantine_bound_to_an_older_generation_is_released_and_the_gate_proceeds() {
    // §7.4: a fingerprint change clears the quarantine. `persist_generation` writes the vault
    // before it updates the store, so a crash between the two leaves a stale quarantine over a
    // newer generation; the gate heals it (§11.2 step 1) instead of reporting it dead.
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a3"));
    fx.quarantine(&a, "invalid_grant", "sha256:an-older-generation");
    assert_ne!(vault_fp(&fx, &a), "sha256:an-older-generation");
    let out = gate(&fx, &a);
    assert!(matches!(out, GateOutcome::Refreshed(_)), "{out:?}");
    assert_eq!(token_requests(&fx), 1, "the request is sent");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a3"));
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    let events = fx.engine.store().unwrap().events().unwrap();
    assert_eq!(
        events.iter().filter(|e| e.kind == "unquarantine").count(),
        1,
        "{events:?}"
    );
}

#[test]
fn the_live_account_s_quarantine_holds_while_its_live_credential_is_bound() {
    // §7.4: the active account's quarantine holds while either the live credential or the
    // vault matches `quarantine_fp`. The vault has moved on; the live credential has not.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a"); // live: rt-a
    let bound = vault_fp(&fx, &a);
    fx.quarantine(&a, "invalid_grant", &bound);
    fx.put_vault(&a, &credential("a@x.co", "rt-a2"));
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::InvalidGrant)
    ));
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("invalid_grant".into()), Some(bound))
    );
    // Once Claude Code has rotated the live credential too, neither copy is bound: the gate
    // releases it, and leaves the live token to §7.5.
    fx.rotate_live("rt-a3");
    assert!(matches!(gate(&fx, &a), GateOutcome::Owned(OwnedBy::Live)));
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn with_the_live_identity_unreadable_the_live_credential_still_decides() {
    // An unreadable live identity may be this account's (§4.3), so the live credential is
    // compared as for the live account: still the bound generation, it holds the quarantine;
    // another generation does not.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a"); // live: rt-a
    let bound = vault_fp(&fx, &a);
    fx.quarantine(&a, "invalid_grant", &bound);
    fx.put_vault(&a, &credential("a@x.co", "rt-a2"));
    fs::write(fx.paths().global_config, "{\n  \"oauthAccount\": ").unwrap(); // torn
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::InvalidGrant)
    ));
    assert!(quarantine_of(&fx, &a).0.is_some());
    fx.rotate_live("rt-a3");
    assert!(matches!(gate(&fx, &a), GateOutcome::Owned(OwnedBy::Live)));
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_token_another_process_already_refreshed_is_returned_without_a_request() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-never-sent"));
    let snapshot = fx.vault_bytes(&a).unwrap();
    let mut elsewhere: Value = serde_json::from_slice(&snapshot).unwrap();
    elsewhere["claudeAiOauth"]["accessToken"] = json!("at-refreshed-elsewhere");
    elsewhere["claudeAiOauth"]["refreshToken"] = json!("rt-a2");
    elsewhere["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms() + 3_600_000);
    fx.put_vault(&a, elsewhere.to_string().as_bytes());
    let out = fx
        .engine
        .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
        .unwrap();
    let GateOutcome::AlreadyFresh(bytes) = out else {
        panic!("expected AlreadyFresh, got {out:?}")
    };
    assert_eq!(bytes, elsewhere.to_string().into_bytes());
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn invalid_grant_quarantines_bound_to_the_generation_sent() {
    let fx = Fx::new();
    let a = due(&fx);
    let sent = vault_fp(&fx, &a);
    fx.script_token_error(400, "invalid_grant");
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::InvalidGrant)
    ));
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("invalid_grant".into()), Some(sent))
    );
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a"),
        "the vault is kept"
    );
    let events = fx.engine.store().unwrap().events().unwrap();
    assert!(events.iter().any(|e| e.kind == "quarantine"), "{events:?}");
}

#[test]
fn a_credential_without_a_refresh_token_is_dead_without_a_request() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-never-sent"));
    let blob = json!({"claudeAiOauth": {"accessToken": "at-only", "expiresAt": fx.clock.now_ms() + 60_000}});
    fx.put_vault(&a, blob.to_string().as_bytes());
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::NoRefreshToken)
    ));
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("no_refresh_token".into()), Some(vault_fp(&fx, &a)))
    );
}

#[test]
fn invalid_client_is_systemic_and_never_a_strike() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_token_error(400, "invalid_client");
    assert!(matches!(gate(&fx, &a), GateOutcome::Systemic(_)));
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn transport_failures_are_transient_and_change_nothing() {
    type Script = Box<dyn Fn(&Fx)>;
    let cases: [(&str, Script); 3] = [
        (
            "pre-send",
            Box::new(|fx| {
                fx.http.push(
                    Method::Post,
                    &Fx::endpoints().token,
                    Err(HttpError::PreSend("dns".into())),
                )
            }),
        ),
        (
            "ambiguous",
            Box::new(|fx| {
                fx.http.push(
                    Method::Post,
                    &Fx::endpoints().token,
                    Err(HttpError::Ambiguous("reset".into())),
                )
            }),
        ),
        (
            "http-500",
            Box::new(|fx| {
                fx.http.push_json(
                    Method::Post,
                    &Fx::endpoints().token,
                    500,
                    json!({"error": "server_error"}),
                )
            }),
        ),
    ];
    for (want, script) in cases {
        let fx = Fx::new();
        let a = due(&fx);
        script(&fx);
        match gate(&fx, &a) {
            GateOutcome::Transient { kind, rescued } => {
                assert_eq!((kind.as_str(), rescued), (want, false))
            }
            other => panic!("{want}: {other:?}"),
        }
        assert_eq!(quarantine_of(&fx, &a), (None, None), "{want}");
        assert_eq!(
            fx.vault_refresh_token(&a).as_deref(),
            Some("rt-a"),
            "{want}"
        );
    }
}

/// `a` as `due` makes it, but in organization `org-mine`, so an organization can disagree.
fn due_in_org(fx: &Fx) -> AccountId {
    common::splice_oauth_account(
        &fx.paths().global_config,
        &json!({"emailAddress": "a@x.co", "organizationUuid": "org-mine",
                "organizationName": null, "accountUuid": "uuid-a@x.co"}),
    );
    fx.set_live_credential(Fx::credential_json("a@x.co", "rt-a").to_string().as_bytes());
    let a = fx.engine.add_live(fx.add_options()).unwrap().account.id;
    fx.add("b@x.co", "rt-b");
    fx.expire_access(&a);
    a
}

/// Every file in `displaced/`, parsed.
fn displaced(fx: &Fx) -> Vec<Value> {
    fx.displaced()
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect()
}

#[test]
fn a_successor_naming_another_account_is_displaced_and_quarantined() {
    let fx = Fx::new();
    let a = due(&fx);
    let sent = vault_fp(&fx, &a);
    fx.http.push_json(
        Method::Post,
        &Fx::endpoints().token,
        200,
        json!({
            "access_token": "at-a2",
            "refresh_token": "rt-a2",
            "expires_in": 28800,
            "scope": "user:inference user:profile",
            "account": {"uuid": "uuid-someone-else", "email_address": "else@x.co"},
            "organization": {"uuid": ""}
        }),
    );
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::IdentityConflict)
    ));
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a"),
        "another account's token never enters this account's vault (§7.3 step 6)"
    );
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("identity_conflict".into()), Some(sent)),
        "bound to the generation that was sent, which the vault still holds"
    );
    let kept = displaced(&fx);
    assert_eq!(
        kept.len(),
        1,
        "the successor is never discarded: it is displaced"
    );
    assert_eq!(kept[0]["claudeAiOauth"]["refreshToken"], "rt-a2");
    assert!(
        !fx.env.data_dir().join("rescue").exists(),
        "never an adoptable rescue"
    );
}

#[test]
fn an_organization_alone_that_disagrees_is_a_conflict() {
    // §7.4: either the uuid or the organization is enough; this reply names no account.
    let fx = Fx::new();
    let a = due_in_org(&fx);
    let sent = vault_fp(&fx, &a);
    fx.http.push_json(
        Method::Post,
        &Fx::endpoints().token,
        200,
        json!({"access_token": "at-a2", "refresh_token": "rt-a2", "expires_in": 28800,
               "organization": {"uuid": "org-other"}}),
    );
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Dead(QuarantineReason::IdentityConflict)
    ));
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("identity_conflict".into()), Some(sent))
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(displaced(&fx).len(), 1);
}

#[test]
fn a_successor_whose_response_names_no_owner_is_not_a_conflict() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.http.push_json(
        Method::Post,
        &Fx::endpoints().token,
        200,
        json!({"access_token": "at-a2", "refresh_token": "rt-a2", "expires_in": 28800}),
    );
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(quarantine_of(&fx, &a), (None, None));
}

#[test]
fn an_unreadable_or_absent_vault_is_transient_without_a_request() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let snapshot = fx.vault_bytes(&a).unwrap();
    fx.kc.set_unreadable(SERVICE, a.as_str(), true);
    let out = fx
        .engine
        .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
        .unwrap();
    assert!(
        matches!(&out, GateOutcome::Transient { kind, rescued: false } if kind == "vault-unreadable"),
        "{out:?}"
    );
    fx.kc.set_unreadable(SERVICE, a.as_str(), false);
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    let out = fx
        .engine
        .refresh_stored(fx.cc.as_ref(), &a, &snapshot)
        .unwrap();
    assert!(
        matches!(&out, GateOutcome::Transient { kind, rescued: false } if kind == "vault-absent"),
        "{out:?}"
    );
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn an_unreadable_rescue_blocks_the_request() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let dir = fx.env.data_dir().join("rescue");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{a}-0-000000000000.json")), "not json").unwrap();
    let out = gate(&fx, &a);
    assert!(
        matches!(&out, GateOutcome::Transient { kind, rescued: false } if kind == "rescue-unreadable"),
        "{out:?}"
    );
    assert_eq!(token_requests(&fx), 0);
}

#[cfg(feature = "test-hooks")]
mod hooked {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::common::mutation_lock_free;

    #[test]
    fn only_the_account_lock_is_held_across_the_request() {
        let fx = Fx::new();
        let a = due(&fx);
        fx.script_refresh(Some("rt-a2"));
        let checked = Arc::new(AtomicBool::new(false));
        let (env, id, refresh_lock, flag) = (
            fx.env.clone(),
            a.clone(),
            fx.paths().refresh_lock,
            checked.clone(),
        );
        fx.engine.on_point(
            "gate-before-request",
            Box::new(move || {
                assert!(
                    mutation_lock_free(&env),
                    "the mutation lock is never held (§4.3)"
                );
                assert!(!refresh_lock.exists(), "no CC lock is held (§4.3)");
                assert!(
                    AccountLock::try_acquire(&env, &id).unwrap().is_none(),
                    "the account lock is held across the request (§7.3 step 1)"
                );
                flag.store(true, Ordering::SeqCst);
            }),
        );
        assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
        assert!(checked.load(Ordering::SeqCst));
    }

    #[test]
    fn invalid_grant_after_the_lineage_moved_is_not_a_strike() {
        let fx = Fx::new();
        let a = due(&fx);
        fx.script_token_error(400, "invalid_grant");
        let mut moved: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
        moved["claudeAiOauth"]["refreshToken"] = json!("rt-a-written-meanwhile");
        let (kc, id, bytes) = (fx.kc.clone(), a.clone(), moved.to_string().into_bytes());
        fx.engine.on_point(
            "gate-after-response",
            Box::new(move || kc.put(SERVICE, id.as_str(), &bytes)),
        );
        let out = gate(&fx, &a);
        assert!(
            matches!(&out, GateOutcome::Transient { kind, rescued: false } if kind == "refresh-failed"),
            "{out:?}"
        );
        assert_eq!(quarantine_of(&fx, &a), (None, None));
    }
}
