mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::{
    Fx, block_rescue, prev_refresh_token, quarantine_of, rescue_files, token_requests,
    unblock_rescue,
};
use serde_json::{Value, json};
use tagteam_cc::ItemKind;
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::active::{ActiveOutcome, ActiveTrigger};
use tagteam_engine::oracle::{CachingOracle, HttpOracle};
use tagteam_engine::quarantine::QuarantineReason;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::http::Method;
use tagteam_provider::{Clock, Provider};

fn bytes(v: &Value) -> Vec<u8> {
    v.to_string().into_bytes()
}

fn cred(email: &str, rt: &str) -> Vec<u8> {
    bytes(&Fx::credential_json(email, rt))
}

fn active(fx: &Fx, trigger: ActiveTrigger) -> Result<ActiveOutcome, EngineError> {
    fx.engine.refresh_active(&fx.provider(), trigger)
}

/// The live access token as §7.2 counts it expired: `now + 5 min ≥ expiresAt`. Only
/// `expiresAt` changes, so the live generation (its refresh token) is still the vault's.
fn expire_live(fx: &Fx) {
    let mut v = fx.live_credential().unwrap();
    v["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms());
    fx.set_live_credential(&bytes(&v));
}

fn fp(fx: &Fx, secret: &[u8]) -> String {
    fx.cc.fingerprint(secret).unwrap().as_str().to_owned()
}

fn quarantine_reason(fx: &Fx, id: &AccountId) -> Option<String> {
    quarantine_of(fx, id).0
}

#[test]
fn an_expired_live_token_is_refreshed_persisted_and_published() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_refresh(Some("rt-a2"));

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::Refreshed);
    assert_eq!(token_requests(&fx), 1);
    let sent = fx.http.requests().into_iter().next().unwrap();
    let body: Value = serde_json::from_slice(sent.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["grant_type"], "refresh_token");
    assert_eq!(body["refresh_token"], "rt-a");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    // The machine-shared keys stay the machine's (§9.4 step 5).
    assert_eq!(
        fx.live_credential().unwrap()["mcpOAuth"],
        json!({"srv": {"token": "machine-shared"}})
    );
    assert!(fx.displaced().is_empty());
    assert!(!fx.paths().refresh_lock.exists() && !fx.paths().config_lock.exists());
}

#[test]
fn a_live_token_neither_expired_nor_rejected_needs_no_request() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let before = fx.kc.items();

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: false });
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.kc.items(), before);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn a_locally_valid_token_the_server_rejected_is_refreshed() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let live = cred("a@x.co", "rt-a");
    let rejected = fx.cc.access_fingerprint(&live).unwrap().as_str().to_owned();

    // A rejection of some other access token says nothing about this one.
    let stale = ActiveTrigger::Rejected {
        access_fp: fx
            .cc
            .access_fingerprint(&cred("a@x.co", "rt-old"))
            .unwrap()
            .as_str()
            .to_owned(),
    };
    assert_eq!(
        active(&fx, stale).unwrap(),
        ActiveOutcome::NotNeeded { reconciled: false }
    );
    assert_eq!(token_requests(&fx), 0);

    fx.script_refresh(Some("rt-a2"));
    let out = active(
        &fx,
        ActiveTrigger::Rejected {
            access_fp: rejected,
        },
    )
    .unwrap();
    assert_eq!(out, ActiveOutcome::Refreshed);
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
}

#[test]
fn row_vault_prev_self_heals_the_live_store_from_the_vault() {
    // An earlier pass reached the vault but not the live store.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.kc.put(SERVICE, a.as_str(), &cred("a@x.co", "rt-b"));
    fx.kc
        .put(SERVICE, &format!("{a}.prev"), &cred("a@x.co", "rt-a"));

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-b"));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn row_a_published_rescue_is_written_to_the_vault() {
    // Published to the live store, but the vault write failed.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let vault = fx.vault_bytes(&a).unwrap();
    fx.rotate_live("rt-r");
    let live = bytes(&fx.live_credential().unwrap());
    fx.plant_rescue(&a, &fp(&fx, &vault), &live);

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-r"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a"));
    assert_eq!(rescue_files(&fx), 0, "its writes are verified: retired");
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn row_in_step_publishes_a_rescue_that_succeeds_the_vault() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let vault = fx.vault_bytes(&a).unwrap();
    fx.plant_rescue(&a, &fp(&fx, &vault), &cred("a@x.co", "rt-s"));

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-s"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-s"));
    assert_eq!(rescue_files(&fx), 0);
}

#[test]
fn row_a_cc_rotation_after_a_published_rescue_is_adopted_and_retires_the_rescue() {
    // §15.2: the rescue rt-r was published, then CC rotated rt-r to rt-c. The live store is
    // where the lineage advances (§7.5 step 3), so rt-c is newest and the rescue is superseded.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let vault = fx.vault_bytes(&a).unwrap();
    fx.rotate_live("rt-r");
    fx.plant_rescue(&a, &fp(&fx, &vault), &bytes(&fx.live_credential().unwrap()));
    fx.rotate_live("rt-c");

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-c"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-c"));
    assert_eq!(rescue_files(&fx), 0);
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn row_a_cc_rotation_is_adopted_then_refreshed_when_expired() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.rotate_live("rt-c");
    expire_live(&fx);
    fx.script_refresh(Some("rt-c2"));

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::Refreshed);
    let sent = fx.http.requests().into_iter().next().unwrap();
    let body: Value = serde_json::from_slice(sent.body.as_deref().unwrap()).unwrap();
    assert_eq!(
        body["refresh_token"], "rt-c",
        "the newest generation is the one sent"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-c2"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-c"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-c2"));
}

#[test]
fn an_access_token_only_live_blob_never_replaces_the_vault_refresh_token() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let blob = json!({"claudeAiOauth": {"accessToken": "at-only", "expiresAt": fx.clock.now_ms()}});
    fx.set_live_credential(&bytes(&blob));

    let err = active(&fx, ActiveTrigger::Expired).unwrap_err();

    assert_eq!(err.kind(), "relogin-required");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_dead_verdict_quarantines_the_account_bound_to_the_generation_sent() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_token_error(400, "invalid_grant");

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::Dead(QuarantineReason::InvalidGrant));
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert_eq!(row.quarantine_reason.as_deref(), Some("invalid_grant"));
    assert_eq!(
        row.quarantine_fp.as_deref(),
        Some(fp(&fx, &cred("a@x.co", "rt-a")).as_str())
    );
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn a_transient_failure_changes_nothing() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    let before = fx.kc.items();

    // Nothing is scripted: the request is never sent (`PreSend`).
    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(
        out,
        ActiveOutcome::Transient {
            kind: "pre-send".into()
        }
    );
    assert_eq!(fx.kc.items(), before);
    assert_eq!(quarantine_reason(&fx, &a), None);
}

#[test]
fn a_failed_vault_write_rescues_the_successor_and_still_publishes_it() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(SERVICE, true);

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(
        out,
        ActiveOutcome::Refreshed,
        "rescue/ is tagteam's storage too"
    );
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(rescue_files(&fx), 1);

    // The next pass finds the rescue's generation live and writes it to the vault.
    fx.kc.set_fail_write(SERVICE, false);
    let out = active(&fx, ActiveTrigger::Expired).unwrap();
    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(rescue_files(&fx), 0);
    assert_eq!(token_requests(&fx), 1);
}

#[test]
fn when_both_writes_fail_but_the_live_store_takes_it_nothing_is_lost() {
    // §7.5 step 5: CC holds the successor, so it is not lost and nothing is quarantined; the
    // next pass adopts it into the vault (step 3's CC-rotation row).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(SERVICE, true);
    block_rescue(&fx);

    let out = active(&fx, ActiveTrigger::Expired).unwrap();
    unblock_rescue(&fx);

    assert_eq!(out, ActiveOutcome::PublishedOnly);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    assert_eq!(
        quarantine_reason(&fx, &a),
        None,
        "a successor CC holds is not lost"
    );

    fx.kc.set_fail_write(SERVICE, false);
    let out = active(&fx, ActiveTrigger::Expired).unwrap();
    assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(token_requests(&fx), 1);
}

/// A 200 token reply whose owner is another account (§7.4).
fn reply_for_someone_else(fx: &Fx) {
    fx.http.push_json(
        Method::Post,
        &Fx::endpoints().token,
        200,
        json!({
            "access_token": "at-z", "refresh_token": "rt-z", "expires_in": 28800,
            "account": {"uuid": "uuid-z@x.co", "email_address": "z@x.co"},
            "organization": {"uuid": ""}
        }),
    );
}

#[test]
fn a_token_response_naming_another_account_is_displaced_never_stored_or_published() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let sent = fp(&fx, &cred("a@x.co", "rt-a"));
    expire_live(&fx);
    reply_for_someone_else(&fx);

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::Dead(QuarantineReason::IdentityConflict));
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert_eq!(row.quarantine_reason.as_deref(), Some("identity_conflict"));
    assert_eq!(
        row.quarantine_fp.as_deref(),
        Some(sent.as_str()),
        "bound to the generation sent"
    );
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a"),
        "another account's token never enters this account's vault (§7.3 step 6)"
    );
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-a"),
        "nor the live store"
    );
    let kept: Vec<Value> = fx
        .displaced()
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    assert_eq!(
        kept.len(),
        1,
        "the successor is never discarded: it is displaced"
    );
    assert_eq!(kept[0]["claudeAiOauth"]["refreshToken"], "rt-z");
    assert_eq!(rescue_files(&fx), 0);
}

#[test]
fn a_lost_conflicting_successor_is_reported_as_unpersisted() {
    // Persistence loss takes precedence over the conflict (§7.3 step 6).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    reply_for_someone_else(&fx);
    let dir = fx.env.data_dir().join("displaced");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();

    let out = active(&fx, ActiveTrigger::Expired).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(out, ActiveOutcome::Unpersisted);
    assert_eq!(
        quarantine_reason(&fx, &a).as_deref(),
        Some("identity_conflict")
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
}

#[test]
fn a_quarantined_active_token_is_never_sent_again() {
    // §7.4: after `invalid_grant`, the quarantine is bound to the live (and vault) generation,
    // so a second call sends nothing.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_token_error(400, "invalid_grant");
    for _ in 0..2 {
        assert_eq!(
            active(&fx, ActiveTrigger::Expired).unwrap(),
            ActiveOutcome::Dead(QuarantineReason::InvalidGrant)
        );
    }
    assert_eq!(
        token_requests(&fx),
        1,
        "the quarantined generation is never sent again"
    );
}

#[test]
fn an_unreadable_prev_refuses_before_any_write_or_request() {
    // Live A, vault B, `.prev` unreadable. `.prev` may be A (an earlier pass never published
    // B), so A must not be adopted over B as if CC had rotated it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.kc.put(SERVICE, a.as_str(), &cred("a@x.co", "rt-b"));
    fx.kc
        .put(SERVICE, &format!("{a}.prev"), &cred("a@x.co", "rt-a"));
    fx.kc.set_unreadable(SERVICE, &format!("{a}.prev"), true);
    expire_live(&fx);

    let err = active(&fx, ActiveTrigger::Expired).unwrap_err();

    assert_eq!(err.kind(), "unreadable", "{err}");
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-b"),
        "B is never overwritten"
    );
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(token_requests(&fx), 0);
}

/// Live A, vault B and `.prev` A: an earlier pass persisted B but never published it. B's
/// access token expires at `b_expires`.
fn unpublished_b(fx: &Fx, a: &AccountId, b_expires: i64) {
    let mut b = Fx::credential_json("a@x.co", "rt-b");
    b["claudeAiOauth"]["expiresAt"] = json!(b_expires);
    fx.kc.put(SERVICE, a.as_str(), &bytes(&b));
    fx.kc
        .put(SERVICE, &format!("{a}.prev"), &cred("a@x.co", "rt-a"));
}

#[test]
fn an_expired_recovered_generation_is_published_before_it_is_refreshed() {
    // B is published first (the self-heal), then refreshed to C in a second pass, so the live
    // store never falls two generations behind.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    unpublished_b(&fx, &a, fx.clock.now_ms());
    fx.script_refresh(Some("rt-c"));

    let out = active(&fx, ActiveTrigger::Expired).unwrap();

    assert_eq!(out, ActiveOutcome::Refreshed);
    let sent: Value =
        serde_json::from_slice(fx.http.requests()[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(
        sent["refresh_token"], "rt-b",
        "the recovered generation is the one sent"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-c"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-b"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-c"));
    assert_eq!(token_requests(&fx), 1);
}

#[test]
fn a_self_heal_that_cannot_publish_sends_nothing_and_says_so() {
    // CC holds its config lock, so B cannot reach the live store. Whether or not B needs a
    // refresh, nothing is sent, and the outcome is `PersistedNotPublished`, never `NotNeeded`.
    for b_expired in [true, false] {
        let fx = Fx::with_lock_timeout(Duration::from_millis(300));
        let a = fx.add("a@x.co", "rt-a");
        let now = fx.clock.now_ms();
        unpublished_b(&fx, &a, if b_expired { now } else { now + 3_600_000 });
        fs::create_dir(fx.paths().config_lock).unwrap(); // CC holds it, freshly

        let out = active(&fx, ActiveTrigger::Expired).unwrap();
        fs::remove_dir(fx.paths().config_lock).unwrap();

        assert_eq!(
            out,
            ActiveOutcome::PersistedNotPublished,
            "B expired: {b_expired}"
        );
        assert_eq!(token_requests(&fx), 0, "B expired: {b_expired}");
        assert_eq!(
            fx.live_refresh_token().as_deref(),
            Some("rt-a"),
            "B expired: {b_expired}"
        );
        assert_eq!(
            fx.vault_refresh_token(&a).as_deref(),
            Some("rt-b"),
            "B expired: {b_expired}"
        );
    }
}

#[test]
fn a_degraded_live_read_is_refused_before_any_request() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    let (svc, acct) = fx.live_item(ItemKind::OAuth);
    fx.kc.set_unreadable(&svc, &acct, true);
    fs::write(fx.paths().credentials_file, cred("a@x.co", "rt-a")).unwrap();

    let err = active(&fx, ActiveTrigger::Expired).unwrap_err();

    assert_eq!(err.kind(), "degraded-read");
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn an_unmanaged_live_login_is_refused() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fx.login("x@y.co", "rt-x");

    let err = active(&fx, ActiveTrigger::Expired).unwrap_err();

    assert_eq!(err.kind(), "invalid-input");
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn an_oracle_naming_someone_else_refuses_without_adopting() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.rotate_live("rt-c");
    fx.oracle.set(Some(
        fx.cc.parse_identity(&Fx::oauth_account("z@x.co")).unwrap(),
    ));

    let err = active(&fx, ActiveTrigger::Expired).unwrap_err();

    assert_eq!(err.kind(), "foreign-credential");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(token_requests(&fx), 0);
}

/// The oracle's profile requests the fixture's scripted port has seen.
fn profile_requests(fx: &Fx) -> usize {
    fx.http.count(Method::Get, &Fx::endpoints().profile)
}

#[test]
fn an_expired_call_leaves_no_cached_skip_to_shadow_a_later_corroboration() {
    // Task 7's carried check: `CachingOracle` remembers a no-answer under the lineage
    // fingerprint. An active refresh of an expired token never asks the oracle, so it caches
    // nothing; a later call for the same refresh token (a reply that keeps the lineage) with a
    // valid access token still gets its own corroboration.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let engine = fx.engine_with_oracle(Arc::new(CachingOracle::new(HttpOracle::new(
        fx.http.clone(),
        fx.clock.clone(),
    ))));
    expire_live(&fx);
    fx.script_refresh(None);

    let out = engine
        .refresh_active(&fx.provider(), ActiveTrigger::Expired)
        .unwrap();
    assert_eq!(out, ActiveOutcome::Refreshed);
    assert_eq!(
        profile_requests(&fx),
        0,
        "an expired token shows nothing to ask about"
    );
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-a"),
        "the lineage is kept"
    );

    let rejected = fx
        .cc
        .access_fingerprint(&bytes(&fx.live_credential().unwrap()))
        .unwrap()
        .as_str()
        .to_owned();
    // A scripted route's last reply repeats, so drop the first refresh's before queueing more.
    fx.http.clear();
    fx.script_profile("a@x.co");
    fx.script_refresh(Some("rt-a2"));
    let out = engine
        .refresh_active(
            &fx.provider(),
            ActiveTrigger::Rejected {
                access_fp: rejected,
            },
        )
        .unwrap();

    assert_eq!(out, ActiveOutcome::Refreshed);
    assert_eq!(
        profile_requests(&fx),
        1,
        "corroborated, not answered from a cached skip"
    );
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};

    use super::*;

    #[test]
    fn only_the_credential_locks_are_held_across_the_request() {
        let fx = Fx::new();
        fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        let seen = Arc::new(Mutex::new(None));
        let (refresh, config, record) = (
            fx.paths().refresh_lock,
            fx.paths().config_lock,
            seen.clone(),
        );
        fx.engine.on_point(
            "active-before-request",
            Box::new(move || {
                *record.lock().unwrap() = Some((refresh.is_dir(), config.exists()));
            }),
        );

        assert_eq!(
            active(&fx, ActiveTrigger::Expired).unwrap(),
            ActiveOutcome::Refreshed
        );
        assert_eq!(
            *seen.lock().unwrap(),
            Some((true, false)),
            "CC's refresh lock is held; its config lock is not (§4.3)"
        );
    }

    #[test]
    fn a_lock_taken_over_during_the_request_persists_but_never_publishes() {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        let refresh = fx.paths().refresh_lock;
        let lock_dir = refresh.clone();
        // A takeover rewrites the lock directory's mtime (§9.1 compromise detection).
        fx.engine.on_point(
            "active-after-response",
            Box::new(move || {
                fs::File::open(&lock_dir)
                    .unwrap()
                    .set_modified(SystemTime::now() + Duration::from_secs(60))
                    .unwrap();
            }),
        );

        let out = active(&fx, ActiveTrigger::Expired).unwrap();

        assert_eq!(out, ActiveOutcome::PersistedNotPublished);
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
        assert_eq!(
            fx.live_refresh_token().as_deref(),
            Some("rt-a"),
            "not published"
        );
        assert!(
            refresh.is_dir(),
            "a lock taken over is left to its new holder"
        );

        // The next pass finds the vault ahead of the live store and self-heals it.
        fs::remove_dir(&refresh).unwrap();
        fx.engine.on_point("active-after-response", Box::new(|| {}));
        let out = active(&fx, ActiveTrigger::Expired).unwrap();
        assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
        assert_eq!(token_requests(&fx), 1);
    }

    /// Makes the credential lock look taken over once the response arrives (§9.1), so the
    /// successor is never published.
    fn take_over_after_response(fx: &Fx) -> PathBuf {
        let refresh = fx.paths().refresh_lock;
        let lock_dir = refresh.clone();
        fx.engine.on_point(
            "active-after-response",
            Box::new(move || {
                fs::File::open(&lock_dir)
                    .unwrap()
                    .set_modified(SystemTime::now() + Duration::from_secs(60))
                    .unwrap();
            }),
        );
        refresh
    }

    #[test]
    fn a_successor_held_nowhere_is_unpersisted_and_quarantines_the_account() {
        // §7.5 step 5, as amended: the vault and rescue/ both fail, and the lock was taken
        // over, so the live store is not written either. The consumed generation must never be
        // sent again: `successor_lost`, bound to it (§7.4).
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let sent = fp(&fx, &cred("a@x.co", "rt-a"));
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        fx.kc.set_fail_write(SERVICE, true);
        block_rescue(&fx);
        take_over_after_response(&fx);

        let out = active(&fx, ActiveTrigger::Expired).unwrap();
        unblock_rescue(&fx);

        assert_eq!(out, ActiveOutcome::Unpersisted);
        let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
        assert_eq!(row.quarantine_reason.as_deref(), Some("successor_lost"));
        assert_eq!(row.quarantine_fp.as_deref(), Some(sent.as_str()));
        assert_eq!(
            fx.live_refresh_token().as_deref(),
            Some("rt-a"),
            "never published"
        );
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    }

    #[test]
    fn a_panic_while_publishing_a_successor_held_nowhere_still_records_the_loss() {
        // Once neither the vault nor rescue/ took the successor, the live write is its last
        // home; a panic inside that write must still record the loss (§7.3 step 6).
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let sent = fp(&fx, &cred("a@x.co", "rt-a"));
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        fx.kc.set_fail_write(SERVICE, true);
        block_rescue(&fx);
        fx.engine.fail_at(Some("panic:active-before-publish"));

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            active(&fx, ActiveTrigger::Expired)
        }));
        fx.engine.fail_at(None);
        unblock_rescue(&fx);

        assert!(unwound.is_err(), "the injected panic unwinds");
        let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
        assert_eq!(row.quarantine_reason.as_deref(), Some("successor_lost"));
        assert_eq!(row.quarantine_fp.as_deref(), Some(sent.as_str()));
        assert_eq!(
            fx.live_refresh_token().as_deref(),
            Some("rt-a"),
            "never published"
        );
    }
}
