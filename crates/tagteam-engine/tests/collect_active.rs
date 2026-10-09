//! §8.1's active account: a usage fetch never refreshes the live token itself. An expired or
//! refused live token goes to active-token refresh (§7.5) first, and the fetch goes on only with
//! a usable, different token that §7.5 leaves in the live store.
mod common;

use std::fs;
use std::time::Duration;

use common::{
    Fx, access_fp, crashed_switch, credential, failed, journal, methods, quarantine_of, refused,
    token_requests, usage_bearers, usage_fixture, usage_requests, write_target_credential,
};
use serde_json::{Value, json};
use tagteam_cc::ItemKind;
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::{Clock, Method, MutationGuard, Provider};

/// The live access token as §7.2 counts it expired. Only `expiresAt` changes, so the live
/// generation is still the vault's.
fn expire_live(fx: &Fx) {
    let mut v = fx.live_credential().unwrap();
    v["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms());
    fx.set_live_credential(v.to_string().as_bytes());
}

/// The body of the first token-endpoint request.
fn token_request(fx: &Fx) -> Value {
    let sent = fx
        .http
        .requests()
        .into_iter()
        .find(|r| r.method == Method::Post)
        .unwrap();
    serde_json::from_slice(sent.body.as_deref().unwrap()).unwrap()
}

/// Stamps `id`'s `rejected_fp` with `fp` directly, as an earlier collection's 401 would have.
/// `Store::set_rejected_fp` is fenced by a reservation (Task 8), which these tests do not hold.
fn stamp_rejected(fx: &Fx, id: &tagteam_core::AccountId, fp: &str) {
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "INSERT INTO usage_state (account_id, rejected_fp) VALUES (?1, ?2) \
             ON CONFLICT(account_id) DO UPDATE SET rejected_fp = excluded.rejected_fp",
            rusqlite::params![id.as_str(), fp],
        )
        .unwrap();
}

/// Stamps `id`'s `rejected_fp` with `rt`'s access token, as an earlier 401 would have.
fn refuse(fx: &Fx, id: &tagteam_core::AccountId, rt: &str) {
    stamp_rejected(fx, id, &access_fp(fx, &credential("a@x.co", rt)));
}

#[test]
fn an_expired_live_token_is_refreshed_by_active_token_refresh_before_the_fetch() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(
        methods(&fx),
        [Method::Post, Method::Get],
        "§7.5 first, then the fetch"
    );
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-a2"),
        "published to the live store, which only §7.5 does"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-a2"],
        "the live token, read fresh"
    );
    assert_eq!(usage_requests(&fx), 1);
}

#[test]
fn a_usage_fetch_refreshes_the_live_generation_never_the_vault_s() {
    // CC rotated the live token to rt-c; the vault still holds rt-a, which CC has spent. The
    // gate refuses the live account (§7.3 step 2) and could only ever send the vault's rt-a;
    // §7.5 adopts rt-c first and refreshes that.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.rotate_live("rt-c");
    expire_live(&fx);
    fx.script_refresh(Some("rt-c2"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(token_request(&fx)["refresh_token"], "rt-c");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-c2"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-c2"));
    assert_eq!(usage_bearers(&fx), ["at-rt-c2"]);
}

#[test]
fn a_401_on_a_token_still_valid_locally_goes_to_active_refresh_and_retries_once() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.script_usage(401, refused());
    fx.script_usage(200, usage_fixture());
    fx.script_refresh(Some("rt-a2"));

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(methods(&fx), [Method::Get, Method::Post, Method::Get]);
    assert_eq!(usage_bearers(&fx), ["at-rt-a", "at-rt-a2"]);
    assert_eq!(usage_requests(&fx), 2, "the retry has a slot of its own");
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    assert_eq!(
        fx.usage_state(&a).unwrap().rejected_fp,
        None,
        "success clears it"
    );
}

#[test]
fn a_stuck_rejected_fp_goes_to_active_refresh_before_any_request() {
    // The path Codex found: a 401 stamps the token, and §7.5 cannot replace it this time (its
    // request never leaves). The next collection must not send the refused token first.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.script_usage(401, refused()); // the only usage reply; nothing for the token endpoint

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
    assert_eq!(
        report.warnings,
        ["could not refresh a@x.co (position 1) to read its usage: pre-send"]
    );
    assert_eq!(usage_bearers(&fx), ["at-rt-a"]);
    let stamped = access_fp(&fx, &credential("a@x.co", "rt-a"));
    assert_eq!(
        fx.usage_state(&a).unwrap().rejected_fp.as_deref(),
        Some(stamped.as_str())
    );

    // Past the lease and the backoff.
    fx.http.clear();
    fx.clock.advance_ms(91_000);
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(
        methods(&fx),
        [Method::Post, Method::Get],
        "§7.5 before any request"
    );
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-a2"],
        "at-rt-a was never sent again"
    );
    assert_eq!(fx.usage_state(&a).unwrap().rejected_fp, None);
}

#[test]
#[cfg(feature = "test-hooks")]
fn a_refused_token_that_active_refresh_left_live_is_never_sent() {
    // §7.5 refreshes the refused token but cannot publish the successor (CC holds its config
    // lock): the live store still holds the refused token, so nothing is sent.
    let fx = Fx::with_lock_timeout(Duration::from_millis(300));
    let a = fx.add("a@x.co", "rt-a");
    refuse(&fx, &a, "rt-a");
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(200, usage_fixture());
    // CC takes the lock once the pre-wait (§9.1) is over, before the successor is published.
    let taken = fx.paths().config_lock;
    fx.engine.on_point(
        "active-before-config-lock",
        Box::new(move || fs::create_dir(&taken).unwrap()),
    );
    // And lets go of it before the next mutation lock, whose pre-wait would otherwise wait it out.
    let held = fx.paths().config_lock;
    fx.engine.also_on_point(
        "before-mutation-lock",
        Box::new(move || {
            let _ = fs::remove_dir(&held);
        }),
    );

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("token-expired"))]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a2"),
        "persisted"
    );
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-a"),
        "not published"
    );
    assert!(
        usage_bearers(&fx).is_empty(),
        "the refused token is never sent again"
    );
    assert_eq!(usage_requests(&fx), 0, "its unsent slot went back");
}

#[test]
fn a_dead_active_refresh_records_a_failure_and_sends_no_usage_request() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_token_error(400, "invalid_grant");

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
    assert!(
        report.warnings.is_empty(),
        "Dead shows as relogin_required, not a warning: {:?}",
        report.warnings
    );
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("invalid_grant"));
    assert!(usage_bearers(&fx).is_empty());
    assert_eq!(usage_requests(&fx), 0);
}

#[test]
fn a_systemic_or_unsent_active_refresh_is_a_failure_with_a_warning() {
    let prefix = "could not refresh a@x.co (position 1) to read its usage: ";
    for systemic in [true, false] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        if systemic {
            fx.script_token_error(400, "invalid_client");
        } // otherwise nothing is scripted: the request never leaves (pre-send)

        let report = fx.collect(&[&a]);

        assert_eq!(
            report.outcomes,
            [(a.clone(), failed("refresh-failed"))],
            "systemic: {systemic}"
        );
        assert_eq!(report.warnings.len(), 1, "systemic: {systemic}");
        assert!(
            report.warnings[0].starts_with(prefix),
            "{:?}",
            report.warnings
        );
        if !systemic {
            assert_eq!(report.warnings[0], format!("{prefix}pre-send"));
        }
        assert_eq!(quarantine_of(&fx, &a).0, None, "never a strike: {systemic}");
        assert!(usage_bearers(&fx).is_empty());
        assert_eq!(usage_requests(&fx), 0);
    }
}

#[test]
fn an_error_from_active_refresh_is_a_failure_and_a_warning_never_a_command_error() {
    // §7.5 refuses before any request: live rt-a, vault rt-b, and an unreadable `.prev` that
    // may be rt-a (Task 16's tri-state rule).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.kc
        .put(SERVICE, a.as_str(), &credential("a@x.co", "rt-b"));
    fx.kc
        .put(SERVICE, &format!("{a}.prev"), &credential("a@x.co", "rt-a"));
    fx.kc.set_unreadable(SERVICE, &format!("{a}.prev"), true);
    expire_live(&fx);

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].starts_with("could not refresh a@x.co (position 1) to read its usage: "),
        "{:?}",
        report.warnings
    );
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0);
}

#[test]
fn a_retry_that_is_also_refused_stamps_the_second_token_and_is_a_401() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.script_usage(401, refused());
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(401, refused());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("http-401"))]);
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-a", "at-rt-a2"],
        "one retry, no more"
    );
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(usage_requests(&fx), 2);
    let second = fx.live_credential().unwrap().to_string().into_bytes();
    assert_eq!(
        fx.usage_state(&a).unwrap().rejected_fp.as_deref(),
        Some(access_fp(&fx, &second).as_str()),
        "the stamp moved to the token the retry sent"
    );
}

#[test]
fn active_refresh_can_run_twice_in_one_collection() {
    // Expired, so §7.5 first; the refreshed token is refused, so §7.5 again, then the retry.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(401, refused());
    fx.script_refresh(Some("rt-a3"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(
        methods(&fx),
        [Method::Post, Method::Get, Method::Post, Method::Get]
    );
    assert_eq!(usage_bearers(&fx), ["at-rt-a2", "at-rt-a3"]);
    assert_eq!(usage_requests(&fx), 2);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a3"));
    assert_eq!(fx.usage_state(&a).unwrap().rejected_fp, None);
}

#[test]
fn one_accounts_error_keeps_the_others_outcomes_and_warnings() {
    // C12: the live account's lock cannot be opened (an error); the inactive account's own
    // thread went on, and its outcome stands beside the warning.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let lock = fx.env.data_dir().join(".mutation.lock");
    fs::remove_file(&lock).unwrap();
    fs::create_dir(&lock).unwrap();
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a, &b]);

    assert_eq!(
        report.outcomes,
        [
            (a.clone(), Collected::Recorded),
            (b.clone(), failed("error"))
        ]
    );
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].starts_with("usage for b@x.co (position 2) was not collected: "),
        "{:?}",
        report.warnings
    );
    assert_eq!(usage_bearers(&fx), ["at-rt-a"]);
    assert_eq!(usage_requests(&fx), 1, "b's unsent slot went back");
}

#[test]
fn a_live_credential_the_oracle_gives_to_someone_else_is_recorded_as_foreign() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    refuse(&fx, &a, "rt-a");
    fx.oracle.set(Some(
        fx.cc.parse_identity(&Fx::oauth_account("z@x.co")).unwrap(),
    ));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("foreign-credential"))]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0);
}

/// A live setup token for a fresh account `s`, with `b` managed beside it.
fn live_setup_token(fx: &Fx) -> tagteam_core::AccountId {
    fx.add("b@x.co", "rt-b");
    let s = fx
        .engine
        .add_token(fx.add_token_options("sk-ant-oat01-setup"))
        .unwrap()
        .account
        .id;
    fx.switch_to(&s, false).unwrap();
    fx.http.clear();
    s
}

#[test]
fn a_remembered_refusal_of_a_live_setup_token_is_a_401_and_never_refreshed() {
    // A setup token does not refresh (§7.1): §7.5 would refuse it with an error, so its
    // refused token is an ordinary 401 failure (Decision 11), as on the inactive path, with
    // no request and no warning.
    let fx = Fx::new();
    let s = live_setup_token(&fx);
    let live = fx.live_credential().unwrap().to_string().into_bytes();
    stamp_rejected(&fx, &s, &access_fp(&fx, &live));

    let report = fx.collect(&[&s]);

    assert_eq!(report.outcomes, [(s.clone(), failed("http-401"))]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(
        fx.http.requests().is_empty(),
        "no refresh and no usage request"
    );
    assert_eq!(usage_requests(&fx), 0);
}

#[test]
fn the_first_refusal_of_a_live_setup_token_is_a_401_and_never_refreshed() {
    let fx = Fx::new();
    let s = live_setup_token(&fx);
    fx.script_usage(401, refused());

    let report = fx.collect(&[&s]);

    assert_eq!(report.outcomes, [(s.clone(), failed("http-401"))]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(methods(&fx), [Method::Get], "one usage request, no refresh");
    let live = fx.live_credential().unwrap().to_string().into_bytes();
    assert_eq!(
        fx.usage_state(&s).unwrap().rejected_fp.as_deref(),
        Some(access_fp(&fx, &live).as_str()),
        "the refused token is remembered"
    );
}

#[test]
fn an_unresolved_switch_journal_stops_the_active_collection_before_any_request() {
    // A switch from b to a died after writing a's credential (step 7) but before a's identity
    // (step 8): the live login still names b, the live credential is a's. Recovery cannot take
    // CC's refresh lock, so the journal row stays. Both identity checks pass (still b), so
    // without the journal check b's reservation would send a's token and record a's usage as
    // b's.
    let fx = Fx::with_lock_timeout(Duration::from_millis(300));
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fs::create_dir(fx.paths().refresh_lock).unwrap();
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&b]);
    fs::remove_dir(fx.paths().refresh_lock).unwrap();

    assert!(
        journal(&fx).is_some(),
        "recovery could not finish, so the row is still there"
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(report.outcomes, [(b.clone(), Collected::Dropped)]);
    // Recovery was blocked by CC's lock, not undecided: the warning names the lock and asks
    // for a retry, as every refusal does, and never points at `--force`.
    assert_eq!(
        report.warnings,
        [format!(
            "usage for the live claude-code account was not collected: an interrupted switch for claude-code could not be recovered yet: timed out waiting for the lock {}; retry once Claude Code is idle",
            fx.paths().refresh_lock.display()
        )]
    );
    assert!(!report.warnings[0].contains("--force"));
    assert!(
        !report.warnings[0].contains("@x.co"),
        "the warning names no email"
    );
    assert!(
        fx.http.requests().is_empty(),
        "a's token was not sent as b's"
    );
    assert_eq!(usage_requests(&fx), 0, "the slot went back");
    assert_eq!(
        fx.usage_state(&b).and_then(|s| s.fetched_at),
        None,
        "no reading for b"
    );
    assert_eq!(
        fx.usage_state(&a).and_then(|s| s.fetched_at),
        None,
        "nor for a"
    );
}

#[test]
fn a_switch_journal_recovery_cannot_decide_points_the_warning_at_force() {
    // The same crash, but recovery took CC's locks and could not decide the row: the
    // Keychain holding a's credential cannot be read, and a stale file says b. Only `--force`
    // settles such a row, so that is what the warning says.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    let (svc, acct) = fx.live_item(ItemKind::OAuth);
    fx.kc.set_unreadable(&svc, &acct, true);
    fs::write(
        fx.paths().credentials_file,
        Fx::credential_json("b@x.co", "rt-b").to_string(),
    )
    .unwrap();
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&b]);

    assert!(journal(&fx).is_some(), "recovery could not decide the row");
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(report.outcomes, [(b.clone(), Collected::Dropped)]);
    assert_eq!(
        report.warnings,
        [
            "usage for the live claude-code account was not collected: an interrupted switch for claude-code could not be resolved; run `tagteam switch <account> --force` to settle it"
        ]
    );
    assert!(fx.http.requests().is_empty(), "nothing was sent");
    assert_eq!(usage_requests(&fx), 0, "the slot went back");
    assert_eq!(fx.usage_state(&b).and_then(|s| s.fetched_at), None);
}

#[test]
fn a_lock_file_that_cannot_be_opened_is_the_collections_warning_not_a_silent_drop() {
    // `.mutation.lock` is a directory, so opening it for the flock fails with EISDIR: a
    // `LockError::Io`, which is not the timeout that drops the collection. Without the
    // distinction every `list` would skip the live account for good, with no signal. The
    // error is that account's warning and outcome (C12), never the whole call's.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a"); // live
    let lock = fx.env.data_dir().join(".mutation.lock");
    fs::remove_file(&lock).unwrap();
    fs::create_dir(&lock).unwrap();
    fx.script_usage(200, usage_fixture());

    let report = fx
        .engine
        .collect_usage(CollectMode::OnDemand {
            accounts: vec![a.clone()],
        })
        .expect("an account's error is its warning, not the call's");

    assert_eq!(report.outcomes, [(a.clone(), failed("error"))]);
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].starts_with("usage for a@x.co (position 1) was not collected: "),
        "{:?}",
        report.warnings
    );
    assert!(fx.http.requests().is_empty(), "nothing was sent");
    // A lasting fault must not spend the hourly budget one `list` at a time.
    assert_eq!(usage_requests(&fx), 0, "the unsent slot went back");
}

#[test]
#[ignore = "waits out the real 10 s mutation-lock timeout (MutationGuard::TIMEOUT)"]
fn a_mutation_lock_held_past_its_timeout_drops_the_active_collection() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a"); // live
    fx.script_usage(200, usage_fixture());
    let held = MutationGuard::acquire(&fx.env, Duration::ZERO).unwrap();

    let report = fx.collect(&[&a]);
    drop(held);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Dropped)]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0, "the slot went back");
    assert_eq!(fx.usage_state(&a).and_then(|s| s.fetched_at), None);
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Instant, SystemTime};

    use super::*;

    #[test]
    fn a_live_login_that_moves_during_active_refresh_records_nothing_and_gives_the_slot_back() {
        // §7.5 ran for b's expired live token; by the time its successor is read, the live
        // login names a: that token is not b's. The second identity read drops the collection.
        let fx = Fx::new();
        fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live
        expire_live(&fx);
        fx.script_refresh(Some("rt-b2"));
        fx.script_usage(200, usage_fixture());
        let config = fx.paths().global_config;
        fx.engine.on_point(
            "active-after-response",
            Box::new(move || {
                common::splice_oauth_account(&config, &Fx::oauth_account("a@x.co"));
            }),
        );

        let report = fx.collect(&[&b]);

        assert_eq!(report.outcomes, [(b.clone(), Collected::Dropped)]);
        assert!(usage_bearers(&fx).is_empty(), "no usage request was sent");
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
        assert_eq!(fx.usage_state(&b).and_then(|s| s.fetched_at), None);
    }

    #[test]
    fn an_error_after_active_refresh_kept_its_successor_is_still_only_a_failure() {
        // M2a Task 16's carry-over: §7.5 returns an error after the successor was received and
        // kept (in rescue/). The collection records a failure and warns; the command goes on.
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        fx.engine.fail_at(Some("active-after-response"));

        let result = fx.engine.collect_usage(CollectMode::OnDemand {
            accounts: vec![a.clone()],
        });
        fx.engine.fail_at(None);

        let report = result.expect("a usage failure is never a command error (§8.3)");
        assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
        assert_eq!(
            report.warnings,
            [
                "could not refresh a@x.co (position 1) to read its usage: injected failure at active-after-response"
            ]
        );
        assert_eq!(
            common::rescue_files(&fx),
            1,
            "the successor was kept before the error returned"
        );
        assert!(usage_bearers(&fx).is_empty());
        assert_eq!(usage_requests(&fx), 0);
    }

    #[test]
    fn a_live_successor_held_nowhere_is_a_warning_naming_the_account() {
        // The vault and rescue/ both fail, and a takeover of CC's lock stops the live write:
        // §7.5's `Unpersisted`, quarantined `successor_lost` (§7.5 step 5).
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        expire_live(&fx);
        fx.script_refresh(Some("rt-a2"));
        fx.kc.set_fail_write(SERVICE, true);
        common::block_rescue(&fx);
        let lock_dir = fx.paths().refresh_lock;
        fx.engine.on_point(
            "active-after-response",
            Box::new(move || {
                // A takeover rewrites the lock directory's mtime (§9.1).
                fs::File::open(&lock_dir)
                    .unwrap()
                    .set_modified(SystemTime::now() + Duration::from_secs(60))
                    .unwrap();
            }),
        );

        let report = fx.collect(&[&a]);
        common::unblock_rescue(&fx);
        fx.kc.set_fail_write(SERVICE, false);

        assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
        assert_eq!(
            report.warnings,
            [
                "a@x.co (position 1) needs a new login: a refreshed token was lost while collecting usage"
            ]
        );
        assert!(!report.warnings[0].contains("rt-a"));
        assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("successor_lost"));
        assert!(usage_bearers(&fx).is_empty());
    }

    #[test]
    fn a_switch_waits_for_the_live_read_so_the_account_sends_its_own_token() {
        // §9.4 writes a's credential before a's identity. Were the collector's reads of b's
        // live identity and credential not under the mutation lock, a switch to a finishing
        // between them would have b's reservation send a's token and record a's usage as b's.
        // The hook runs a switch on another thread between the two reads and gives it up to
        // 2 s to finish; under the lock it cannot, and it goes ahead once the reads are done.
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live
        fx.script_usage(200, usage_fixture());
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let request = fx.switch_request(&a, false);
        let switching: Arc<Mutex<Option<JoinHandle<bool>>>> = Arc::default();
        let handle_slot = switching.clone();
        fx.engine.on_point(
            "usage-live-identity-read",
            Box::new(move || {
                let (engine, req) = (other.clone(), request.clone());
                let switch = thread::spawn(move || engine.switch(req).is_ok());
                let deadline = Instant::now() + Duration::from_secs(2);
                while !switch.is_finished() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(20));
                }
                *handle_slot.lock().unwrap() = Some(switch);
            }),
        );

        let report = fx.collect(&[&b]);
        let switch = switching.lock().unwrap().take().expect("the hook ran");
        assert!(
            switch.join().unwrap(),
            "the switch went ahead once the reads were done"
        );

        assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
        assert_eq!(report.outcomes, [(b.clone(), Collected::Recorded)]);
        assert_eq!(
            usage_bearers(&fx),
            ["at-rt-b"],
            "b's reservation sent b's own token"
        );
        assert!(
            fx.usage_state(&b).is_some_and(|s| s.fetched_at.is_some()),
            "the reading is recorded as b's"
        );
        assert_eq!(
            fx.usage_state(&a).and_then(|s| s.fetched_at),
            None,
            "nothing is recorded as a's"
        );
    }

    /// `engine`'s cancel token, set from inside the named hook as a signal handler would set it.
    fn signal_at(engine: &tagteam_engine::Engine, point: &'static str) {
        let cancel = engine.cancel().clone();
        engine.on_point(point, Box::new(move || cancel.request(libc::SIGINT)));
    }

    /// `id`'s on-demand collection through `engine` ended as SIGINT's interruption.
    fn assert_interrupted(engine: &tagteam_engine::Engine, id: &tagteam_core::AccountId) {
        let result = engine.collect_usage(CollectMode::OnDemand {
            accounts: vec![id.clone()],
        });
        assert!(
            matches!(
                result,
                Err(tagteam_engine::EngineError::Interrupted(libc::SIGINT))
            ),
            "{result:?}"
        );
    }

    #[test]
    fn a_signal_before_active_token_refresh_records_nothing_for_an_expired_live_token() {
        // §14.1: no §7.5 starts once the token is set. A live setup token cannot refresh, so
        // without the cancellation point at §7.5's entry its expired token would be recorded as
        // a `token-expired` failure, backing the account off for a fetch that never happened.
        let fx = Fx::new();
        let s = live_setup_token(&fx);
        let mut live = fx.live_credential().unwrap();
        live["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms());
        fx.set_live_credential(live.to_string().as_bytes());
        signal_at(&fx.engine, "usage-live-identity-read");

        assert_interrupted(&fx.engine, &s);

        assert_eq!(fx.usage_state(&s), None, "nothing is recorded");
        assert!(fx.http.requests().is_empty());
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
    }

    #[test]
    fn a_signal_before_active_token_refresh_asks_the_oracle_nothing() {
        // §14.1: the live token was refused (`rejected_fp`) but is still valid locally, so §7.5
        // asks the profile oracle about it before taking any lock (§7.6). Once the token is set
        // that request must not leave.
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        refuse(&fx, &a, "rt-a");
        let engine = fx.engine_with_oracle(Arc::new(tagteam_engine::oracle::HttpOracle::new(
            fx.http.clone(),
            fx.clock.clone(),
        )));
        signal_at(&engine, "usage-live-identity-read");

        assert_interrupted(&engine, &a);

        assert!(
            fx.http.requests().is_empty(),
            "no profile, token or usage request"
        );
        assert_eq!(fx.usage_state(&a).and_then(|s| s.last_error), None);
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
    }
}

#[test]
fn a_replaced_live_login_reports_live_replaced_and_names_the_forced_switch() {
    // §7.5 step 2, §8.1, Decision 10: §7.5 stops with `Replaced`, so nothing is refreshed or
    // fetched; the failure is `live-replaced` (unavailable), its slot goes back, and one
    // warning says how to activate the replacement.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let a = fx.add("a@x.co", "rt-a"); // live, position 2
    fx.replace_login(&a, &credential("a@x.co", "rt-new"), "oauth");
    fx.rotate_live("rt-a2");
    expire_live(&fx);
    fx.script_refresh(Some("rt-x"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("live-replaced"))]);
    assert_eq!(
        report.warnings,
        [
            "a@x.co (position 2)'s login was replaced while Claude Code kept the old one; run `tagteam switch 2 --force` to activate the replacement"
        ]
    );
    assert!(
        fx.http.requests().is_empty(),
        "nothing refreshed or fetched"
    );
    assert_eq!(
        usage_requests(&fx),
        0,
        "nothing was sent: the slot went back"
    );
    assert_eq!(
        fx.usage_state(&a).unwrap().last_error.as_deref(),
        Some("live-replaced")
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-new"));
}
