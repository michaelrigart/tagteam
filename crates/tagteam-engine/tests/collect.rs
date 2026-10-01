//! §8.1 and §8.3: the collector's reserve, token, fetch and record phases for inactive
//! accounts (and the live token, read but never refreshed by a fetch), the hourly budget across
//! processes (§8.6, Review Focus 1 and 2), and provider neutrality (§15.2).
mod common;

use std::collections::BTreeSet;
use std::fs;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::{
    API_KEY, FakeFx, Fx, access_fp, block_rescue, credential, due, failed, methods, quarantine_of,
    refused, token_requests, two_accounts, unblock_rescue, usage_bearers, usage_fixture,
    usage_requests,
};
use serde_json::{Value, json};
use tagteam_cc::ItemKind;
use tagteam_cc::usage::normalize;
use tagteam_core::backoff::failure_backoff_s;
use tagteam_core::{AccountId, WindowKind};
use tagteam_engine::Engine;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::collect::{CollectMode, Collected};
use tagteam_engine::store::{Ineligible, UsageStateRow};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::{
    Http, HttpError, HttpRequest, HttpResponse, Keychain, Method, Provider, ScriptedHttp,
};

/// The fixture clock's start, in the seconds the usage tables hold (Decision 1).
const NOW_S: i64 = 1_790_000_000;

fn state(fx: &Fx, id: &AccountId) -> UsageStateRow {
    fx.usage_state(id)
        .expect("the collection wrote a usage_state row")
}

/// Another process's on-demand collection of `id`.
fn collect_on(engine: &Engine, id: &AccountId) -> Vec<(AccountId, Collected)> {
    engine
        .collect_usage(CollectMode::OnDemand {
            accounts: vec![id.clone()],
        })
        .unwrap()
        .outcomes
}

/// Spends `n` of the hourly budget of `id`'s identity, reserved `ago` seconds before now.
fn spend_budget(fx: &Fx, id: &AccountId, n: usize, ago: i64) {
    let key = fx
        .engine
        .store()
        .unwrap()
        .account(id)
        .unwrap()
        .unwrap()
        .identity_key;
    let conn = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    for _ in 0..n {
        conn.execute(
            "INSERT INTO usage_requests(provider, identity_key, at) VALUES (?1, ?2, ?3)",
            rusqlite::params![fx.provider().as_str(), key, NOW_S - ago],
        )
        .unwrap();
    }
}

#[test]
fn an_inactive_account_is_fetched_with_its_stored_token_and_recorded() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(usage_bearers(&fx), ["at-rt-a"]);
    let requests = fx.http.requests();
    assert!(
        requests[0]
            .headers
            .iter()
            .any(|(k, v)| k == "anthropic-beta" && v == "oauth-2025-04-20"),
        "{:?}",
        requests[0]
    );
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(usage_requests(&fx), 1, "the one request holds the one slot");
    let s = state(&fx, &a);
    assert_eq!(s.last_good, Some(normalize(&usage_fixture()).unwrap()));
    assert_eq!(s.fetched_at, Some(NOW_S));
    assert_eq!((s.consecutive_failures, s.last_error), (0, None));
    let next = s.next_poll_at.unwrap();
    assert!((NOW_S + 180..=NOW_S + 660).contains(&next), "{next}");
    let samples = fx
        .engine
        .store()
        .unwrap()
        .usage_samples(&a, None, 0)
        .unwrap();
    let keys: BTreeSet<&str> = samples.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, BTreeSet::from(["5h", "7d", "scoped:Fable"]));
    assert!(samples.iter().all(|(_, s)| s.fetched_at == NOW_S));
}

#[test]
fn an_expired_stored_token_is_refreshed_through_the_gate_before_the_fetch() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(methods(&fx), [Method::Post, Method::Get], "the gate first");
    assert_eq!(usage_bearers(&fx), ["at-rt-a2"]);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(
        fx.live_refresh_token().as_deref(),
        Some("rt-b"),
        "the live login is b's and untouched"
    );
    assert_eq!(usage_requests(&fx), 1);
}

#[test]
fn a_dead_refresh_quarantines_the_account_and_gives_its_slot_back() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_token_error(400, "invalid_grant");

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(
        usage_bearers(&fx).is_empty(),
        "a Dead verdict never reaches the usage endpoint (§8.1)"
    );
    assert_eq!(
        usage_requests(&fx),
        0,
        "a fetch that ended before sending gives its slot back"
    );
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("invalid_grant"));
    let s = state(&fx, &a);
    assert_eq!(s.last_error.as_deref(), Some("refresh-failed"));
    assert_eq!(s.consecutive_failures, 1);
    assert_eq!(
        s.backoff_until,
        Some(NOW_S + failure_backoff_s(1, false, None))
    );
    assert_eq!((s.last_good, s.fetched_at), (None, None));

    // Past the 90 s lease and the backoff, only the quarantine stands (§7.4).
    fx.clock.advance_ms(91_000);
    let report = fx.collect(&[&a]);
    assert_eq!(
        report.outcomes,
        [(a.clone(), Collected::Ineligible(Ineligible::Quarantined))]
    );
    assert_eq!(token_requests(&fx), 1);
    assert!(usage_bearers(&fx).is_empty());
}

#[test]
fn every_other_gate_refusal_is_a_failure_that_sends_nothing() {
    for case in ["busy", "pre-send", "systemic"] {
        let fx = Fx::new();
        let a = due(&fx);
        let held = (case == "busy")
            .then(|| AccountLock::acquire(&fx.env, &a, Duration::from_secs(1)).unwrap());
        if case == "systemic" {
            fx.script_token_error(400, "invalid_client");
        }
        // "pre-send": nothing is scripted for the token endpoint, so its request never leaves.

        let report = fx.collect(&[&a]);
        drop(held);

        assert_eq!(
            report.outcomes,
            [(a.clone(), failed("refresh-failed"))],
            "{case}"
        );
        assert!(usage_bearers(&fx).is_empty(), "{case}");
        assert_eq!(usage_requests(&fx), 0, "{case}");
        assert_eq!(quarantine_of(&fx, &a).0, None, "never a strike: {case}");
        assert_eq!(
            fx.vault_refresh_token(&a).as_deref(),
            Some("rt-a"),
            "{case}"
        );
    }
}

#[test]
fn a_lost_successor_is_a_warning_that_names_the_account_and_no_token() {
    // §8.3: the gate spent rt-a and could store rt-a2 nowhere (§7.3 step 6, `Unpersisted`).
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.kc.set_fail_write(SERVICE, true);
    block_rescue(&fx);

    let report = fx.collect(&[&a]);
    unblock_rescue(&fx);
    fx.kc.set_fail_write(SERVICE, false);

    assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
    assert_eq!(
        report.warnings,
        [
            "a@x.co (position 1) needs a new login: a refreshed token was lost while collecting usage"
        ]
    );
    assert!(!report.warnings[0].contains("rt-a") && !report.warnings[0].contains("at-rt"));
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("successor_lost"));
    assert!(usage_bearers(&fx).is_empty());
    assert_eq!(usage_requests(&fx), 0);
}

/// A way to break an account's stored token.
type Breakage = fn(&Fx, &AccountId);

#[test]
fn a_missing_token_or_an_unreadable_vault_is_recorded_without_sending() {
    let cases: [(&str, Breakage); 3] = [
        ("no-access-token", |fx, a| {
            fx.put_vault(
                a,
                json!({"claudeAiOauth": {"refreshToken": "rt-a"}})
                    .to_string()
                    .as_bytes(),
            )
        }),
        ("vault-absent", |fx, a| {
            fx.kc.delete(SERVICE, a.as_str()).unwrap()
        }),
        ("keychain-unavailable", |fx, a| {
            fx.kc.set_unreadable(SERVICE, a.as_str(), true)
        }),
    ];
    for (kind, break_it) in cases {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        break_it(&fx, &a);
        fx.script_usage(200, usage_fixture());

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), failed(kind))], "{kind}");
        assert!(fx.http.requests().is_empty(), "nothing was sent: {kind}");
        assert_eq!(usage_requests(&fx), 0, "{kind}");
        assert_eq!(state(&fx, &a).last_error.as_deref(), Some(kind));
    }
}

#[test]
fn a_managed_api_key_has_no_usage_and_reserves_nothing() {
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b");
    let k = fx.add_api_key(API_KEY);

    let report = fx.collect(&[&k]);

    assert_eq!(report.outcomes, [(k.clone(), Collected::Unsupported)]);
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0);
    assert_eq!(fx.usage_state(&k), None);
}

#[test]
fn a_401_refreshes_once_through_the_gate_and_retries_under_its_own_slot() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.script_usage(401, refused());
    fx.script_usage(200, usage_fixture());
    fx.script_refresh(Some("rt-a2"));

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(methods(&fx), [Method::Get, Method::Post, Method::Get]);
    assert_eq!(usage_bearers(&fx), ["at-rt-a", "at-rt-a2"]);
    assert_eq!(
        usage_requests(&fx),
        2,
        "the retry is a request like any other (§8.6)"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(state(&fx, &a).rejected_fp, None, "success clears it");
}

#[test]
fn a_second_401_is_recorded_and_the_refused_token_is_never_sent_again() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.script_usage(401, refused()); // the only reply: every usage request is refused
    fx.script_refresh(Some("rt-a2"));

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("http-401"))]);
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-a", "at-rt-a2"],
        "one retry, no more"
    );
    assert_eq!(usage_requests(&fx), 2);
    let refused_fp = access_fp(&fx, &fx.vault_bytes(&a).unwrap());
    assert_eq!(
        state(&fx, &a).rejected_fp.as_deref(),
        Some(refused_fp.as_str())
    );

    // Next time, the refused token goes to the gate before any usage request.
    fx.http.clear();
    fx.clock.advance_ms(91_000);
    fx.script_refresh(Some("rt-a3"));
    fx.script_usage(200, usage_fixture());
    let report = fx.collect(&[&a]);
    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(methods(&fx), [Method::Post, Method::Get]);
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-a3"],
        "at-rt-a2 was never sent again"
    );
    assert_eq!(state(&fx, &a).rejected_fp, None);
}

#[test]
fn a_token_the_gate_just_refreshed_is_not_refreshed_again_after_a_401() {
    // §8.3: at most a gate refresh, a fetch and one retry. The expired token went through the
    // gate; its successor's 401 is recorded, not refreshed and retried.
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(401, refused());
    fx.script_refresh(Some("rt-a3"));
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("http-401"))]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(token_requests(&fx), 1, "one gate refresh in the collection");
    assert_eq!(usage_bearers(&fx), ["at-rt-a2"], "no retry");
    assert_eq!(usage_requests(&fx), 1);
    let refused_fp = access_fp(&fx, &fx.vault_bytes(&a).unwrap());
    assert_eq!(
        state(&fx, &a).rejected_fp.as_deref(),
        Some(refused_fp.as_str())
    );
}

#[test]
fn an_expired_token_that_cannot_be_refreshed_is_token_expired_and_sends_nothing() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    // An access token that has expired and no refresh token to renew it with.
    fx.put_vault(
        &a,
        json!({"claudeAiOauth": {"accessToken": "at-old", "expiresAt": NOW_S * 1000 - 1}})
            .to_string()
            .as_bytes(),
    );

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("token-expired"))]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(
        fx.http.requests().is_empty(),
        "no refresh, no usage request"
    );
    assert_eq!(usage_requests(&fx), 0);
    assert_eq!(state(&fx, &a).last_error.as_deref(), Some("token-expired"));
}

#[test]
fn a_refused_setup_token_is_a_401_on_the_inactive_path_whether_first_or_remembered() {
    // Decision 11: a refusal is an ordinary failure. A setup token does not refresh (§7.1), so
    // neither its first 401 nor a remembered one reaches the gate.
    let fx = Fx::new();
    fx.add("b@x.co", "rt-b"); // live
    let s = fx
        .engine
        .add_token(fx.add_token_options("sk-ant-oat01-setup"))
        .unwrap()
        .account
        .id;
    fx.http.clear();
    fx.script_usage(401, refused());

    let report = fx.collect(&[&s]);

    assert_eq!(report.outcomes, [(s.clone(), failed("http-401"))]);
    assert_eq!(methods(&fx), [Method::Get], "one usage request, no refresh");
    let fp = access_fp(&fx, &fx.vault_bytes(&s).unwrap());
    assert_eq!(state(&fx, &s).rejected_fp.as_deref(), Some(fp.as_str()));

    // Remembered: past the lease and the backoff, nothing is sent at all.
    fx.http.clear();
    fx.clock.advance_ms(91_000);
    let report = fx.collect(&[&s]);
    assert_eq!(report.outcomes, [(s.clone(), failed("http-401"))]);
    assert!(
        fx.http.requests().is_empty(),
        "no refresh, no usage request"
    );
}

#[test]
fn a_429_backs_off_as_retry_after_asks_and_records_when_the_block_lifts() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.script_usage_429("120");

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("http-429"))]);
    let lift = NOW_S + failure_backoff_s(1, true, Some(120.0));
    let s = state(&fx, &a);
    assert_eq!(s.backoff_until, Some(lift));
    assert_eq!(
        s.last_429_at,
        Some(lift),
        "Decision 2: when the backoff lifts"
    );
    assert_eq!(
        usage_requests(&fx),
        1,
        "the request was sent, so its slot stays counted"
    );

    // Past the lease, not past the backoff.
    fx.clock.advance_ms(91_000);
    let report = fx.collect(&[&a]);
    assert_eq!(
        report.outcomes,
        [(a.clone(), Collected::Ineligible(Ineligible::Backoff))]
    );
    assert_eq!(usage_bearers(&fx).len(), 1);
}

#[test]
fn failures_never_touch_the_last_good_reading() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.script_usage(200, usage_fixture());
    assert_eq!(
        fx.collect(&[&a]).outcomes,
        [(a.clone(), Collected::Recorded)]
    );
    let good = state(&fx, &a);

    // Past the reading's plan (at most 660 s) and its lease.
    fx.http.clear();
    fx.clock.advance_ms(700_000);
    fx.script_usage(
        503,
        json!({"type": "error", "error": {"type": "overloaded_error"}}),
    );
    assert_eq!(
        fx.collect(&[&a]).outcomes,
        [(a.clone(), failed("http-503"))]
    );

    fx.http.clear();
    fx.clock.advance_ms(91_000);
    fx.http.push(
        Method::Get,
        &Fx::endpoints().usage,
        Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: b"<html>busy</html>".to_vec(),
        }),
    );
    assert_eq!(
        fx.collect(&[&a]).outcomes,
        [(a.clone(), failed("bad-response"))]
    );

    let s = state(&fx, &a);
    assert_eq!(
        (s.last_good, s.fetched_at),
        (good.last_good, good.fetched_at),
        "§8.3: failure never touches the last good reading"
    );
    assert_eq!(s.consecutive_failures, 2);
    assert_eq!(s.last_error.as_deref(), Some("bad-response"));
}

#[test]
fn a_request_that_never_left_gives_its_slot_back() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.http.push(
        Method::Get,
        &Fx::endpoints().usage,
        Err(HttpError::PreSend("dns lookup failed".into())),
    );

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("pre-send"))]);
    assert_eq!(
        usage_requests(&fx),
        0,
        "the request never left, so its slot is given back"
    );
}

#[test]
fn an_identity_over_its_hourly_budget_is_sent_nothing() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    spend_budget(&fx, &a, 20, 100);
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    let free = NOW_S - 100 + 3660;
    assert_eq!(
        report.outcomes,
        [(a.clone(), Collected::OverBudget { next_free_at: free })]
    );
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 20);
    // §8.6: the fetch reports `over-budget`, even for an account never read (Review Focus 4's
    // re-added account), so its row says so rather than "no data yet".
    let s = state(&fx, &a);
    assert_eq!(
        (
            s.last_good,
            s.fetched_at,
            s.last_attempt_at,
            s.consecutive_failures,
            s.last_error.as_deref(),
            s.backoff_until,
            s.next_poll_at
        ),
        (
            None,
            None,
            Some(NOW_S),
            1,
            Some("over-budget"),
            Some(free),
            Some(free)
        )
    );
    assert_eq!(
        fx.collect(&[&a]).outcomes,
        [(a.clone(), Collected::Ineligible(Ineligible::Backoff))],
        "recorded once per budget period"
    );
    assert_eq!(state(&fx, &a).consecutive_failures, 1);
}

#[test]
fn the_retry_after_a_401_needs_a_slot_of_its_own() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    spend_budget(&fx, &a, 19, 100);
    fx.script_usage(401, refused());
    fx.script_refresh(Some("rt-a2"));

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), failed("over-budget"))]);
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-a"],
        "the retry is never sent without a slot"
    );
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(usage_requests(&fx), 20);
    assert!(
        state(&fx, &a).backoff_until.unwrap() >= NOW_S - 100 + 3660,
        "no retry before a slot frees up"
    );
}

#[test]
fn the_live_account_is_fetched_with_the_live_token() {
    // CC refreshed its access token on its own; the vault still holds the older one.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let mut live = fx.live_credential().unwrap();
    live["claudeAiOauth"]["accessToken"] = json!("at-live");
    fx.set_live_credential(live.to_string().as_bytes());
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
    assert_eq!(usage_bearers(&fx), ["at-live"]);
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_degraded_live_read_is_a_keychain_failure_that_sends_nothing() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let (svc, acct) = fx.live_item(ItemKind::OAuth);
    fx.kc.set_unreadable(&svc, &acct, true);
    fs::write(fx.paths().credentials_file, credential("a@x.co", "rt-a")).unwrap();
    fx.script_usage(200, usage_fixture());

    let report = fx.collect(&[&a]);

    assert_eq!(
        report.outcomes,
        [(a.clone(), failed("keychain-unavailable"))]
    );
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0);
}

/// Holds every request until `want` are in flight at once (or 5 s pass), then answers from
/// `inner`: the collector's requests overlap only if it collects the accounts in parallel.
struct Rendezvous {
    inner: Arc<ScriptedHttp>,
    want: usize,
    /// In flight now, and the most ever in flight at once.
    counts: Mutex<(usize, usize)>,
    arrived: Condvar,
}

impl Http for Rendezvous {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut counts = self.counts.lock().unwrap();
        counts.0 += 1;
        counts.1 = counts.1.max(counts.0);
        self.arrived.notify_all();
        while counts.1 < self.want {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            counts = self.arrived.wait_timeout(counts, left).unwrap().0;
        }
        counts.0 -= 1;
        drop(counts);
        self.inner.send(req)
    }
}

#[test]
fn every_account_is_collected_on_its_own_thread() {
    // §8.3: one thread each, and the command waits for all of them.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c"); // live
    fx.script_usage(200, usage_fixture());
    let probe = Arc::new(Rendezvous {
        inner: fx.http.clone(),
        want: 3,
        counts: Mutex::new((0, 0)),
        arrived: Condvar::new(),
    });
    let engine = fx.engine_with_http(probe.clone());

    let report = engine
        .collect_usage(CollectMode::OnDemand {
            accounts: vec![a.clone(), b.clone(), c.clone()],
        })
        .unwrap();

    assert_eq!(
        report.outcomes,
        [
            (a, Collected::Recorded),
            (b, Collected::Recorded),
            (c, Collected::Recorded)
        ]
    );
    assert_eq!(
        probe.counts.lock().unwrap().1,
        3,
        "all three requests were in flight at once"
    );
    let bearers: BTreeSet<String> = usage_bearers(&fx).into_iter().collect();
    assert_eq!(
        bearers,
        BTreeSet::from(["at-rt-a".to_owned(), "at-rt-b".into(), "at-rt-c".into()])
    );
    assert_eq!(usage_requests(&fx), 3);
}

#[test]
fn two_processes_collecting_at_once_send_one_request() {
    // Review Focus 1: two `tagteam list` at once. Whichever reserves second finds the lease
    // taken or the reading already fresh; the budget counts the one request once.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let other = fx.engine_with_env(fx.env.clone());
    fx.script_usage(200, usage_fixture());

    let outcomes: Vec<Collected> = thread::scope(|s| {
        let mine = s.spawn(|| fx.collect(&[&a]).outcomes);
        let theirs = s.spawn(|| collect_on(&other, &a));
        [mine.join().unwrap(), theirs.join().unwrap()]
            .into_iter()
            .map(|o| o[0].1.clone())
            .collect()
    });

    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == Collected::Recorded)
            .count(),
        1,
        "{outcomes:?}"
    );
    assert!(
        outcomes.iter().all(|o| matches!(
            o,
            Collected::Recorded | Collected::Ineligible(Ineligible::Leased | Ineligible::NotDue)
        )),
        "{outcomes:?}"
    );
    assert_eq!(usage_bearers(&fx).len(), 1);
    assert_eq!(usage_requests(&fx), 1);
}

/// `id`'s stored FakeAgent access token, moved inside §7.2's expiry buffer.
fn make_due(ffx: &FakeFx, id: &AccountId) {
    let mut v: Value = serde_json::from_slice(&ffx.fx.vault_bytes(id).unwrap()).unwrap();
    v["fa"]["expires"] = json!(NOW_S * 1000 + 60_000);
    ffx.fx.put_vault(id, v.to_string().as_bytes());
}

#[test]
fn fake_agent_usage_is_collected_through_the_same_engine_and_claude_code_is_untouched() {
    // §15.2: FakeAgent's usage goes through the same collector, gate and store, with its own
    // endpoint and shapes. The parked M2a check: Claude Code's stored login, row and usage
    // stay exactly as they were.
    let ffx = FakeFx::new();
    let cc = ffx.fx.add("cc@b.co", "rt-cc");
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let bob = ffx.fake_add("bob", "tok-b", "renew-b"); // live
    make_due(&ffx, &alice);
    ffx.fx.http.push_json(
        Method::Post,
        &ffx.fake.renew_url(),
        200,
        json!({"token": "tok-a-2", "renew": "renew-a-2", "expires_in": 3600}),
    );
    ffx.fx.http.push_json(
        Method::Get,
        &ffx.fake.usage_url(),
        200,
        json!({"meters": [
            {"id": "daily", "used": 0.42, "renews": NOW_S + 3600},
            {"id": "monthly", "used": 0.1, "renews": NOW_S + 86_400}
        ]}),
    );
    let store = ffx.engine.store().unwrap();
    let (cc_row, cc_vault) = (store.account(&cc).unwrap(), ffx.fx.vault_bytes(&cc));
    let before = ffx.fx.snapshot();

    let report = ffx
        .engine
        .collect_usage(CollectMode::OnDemand {
            accounts: vec![alice.clone(), bob.clone()],
        })
        .unwrap();
    let after = ffx.fx.snapshot();

    assert_eq!(
        report.outcomes,
        [
            (alice.clone(), Collected::Recorded),
            (bob.clone(), Collected::Recorded)
        ]
    );
    assert_eq!(ffx.fx.http.count(Method::Post, &ffx.fake.renew_url()), 1);
    let sent: BTreeSet<String> = ffx
        .fx
        .http
        .requests()
        .iter()
        .filter(|r| r.url == ffx.fake.usage_url())
        .filter_map(|r| {
            r.headers
                .iter()
                .find(|(k, _)| k == "authorization")
                .and_then(|(_, v)| v.strip_prefix("Fake "))
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(
        sent,
        BTreeSet::from(["tok-a-2".to_owned(), "tok-b".to_owned()])
    );
    let windows = store
        .usage_state(&alice)
        .unwrap()
        .unwrap()
        .last_good
        .unwrap();
    let shape: Vec<(&str, WindowKind)> = windows.iter().map(|w| (w.key.as_str(), w.kind)).collect();
    assert_eq!(
        shape,
        [("daily", WindowKind::Short), ("monthly", WindowKind::Long)]
    );
    assert!((windows[0].pct - 42.0).abs() < 1e-9, "{}", windows[0].pct);

    // Claude Code: nothing sent, nothing stored, nothing changed.
    assert_eq!(ffx.fx.http.count(Method::Get, &Fx::endpoints().usage), 0);
    assert_eq!(ffx.fx.http.count(Method::Post, &Fx::endpoints().token), 0);
    assert_eq!(store.account(&cc).unwrap(), cc_row);
    assert_eq!(ffx.fx.vault_bytes(&cc), cc_vault);
    assert_eq!(store.usage_state(&cc).unwrap(), None);
    ffx.fx.assert_only_surface_changed_for(
        &ffx.fake.identity_surface(&ffx.fx.env),
        &before,
        &after,
        "collecting FakeAgent usage",
    );
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use super::*;

    #[test]
    fn a_second_process_finds_the_lease_taken_while_the_first_fetches() {
        // Review Focus 1, deterministically: the other process reserves while this one holds
        // the lease.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let seen = Arc::new(Mutex::new(None));
        let (engine, id, record) = (other.clone(), a.clone(), seen.clone());
        fx.engine.on_point(
            "usage-reserved",
            Box::new(move || *record.lock().unwrap() = Some(collect_on(&engine, &id))),
        );

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
        assert_eq!(
            *seen.lock().unwrap(),
            Some(vec![(a.clone(), Collected::Ineligible(Ineligible::Leased))])
        );
        assert_eq!(usage_bearers(&fx).len(), 1);
        assert_eq!(usage_requests(&fx), 1);
    }

    #[test]
    fn a_slot_not_sent_within_its_validity_is_replaced_before_sending() {
        // Review Focus 2: suspended for 61 s between reserving and sending (§8.6).
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        let clock = fx.clock.clone();
        fx.engine
            .on_point("usage-reserved", Box::new(move || clock.advance_ms(61_000)));

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
        assert_eq!(
            common::slot_times(&fx),
            [NOW_S + 61],
            "the stale slot went back, and the request went under a fresh one"
        );
        assert_eq!(usage_bearers(&fx).len(), 1);
    }

    #[test]
    fn a_result_recorded_after_another_process_took_the_lease_is_dropped() {
        // Review Focus 2: suspended for 91 s between fetching and recording, past the lease.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let seen = Arc::new(Mutex::new(None));
        let (clock, engine, id, record) =
            (fx.clock.clone(), other.clone(), a.clone(), seen.clone());
        fx.engine.on_point(
            "usage-before-record",
            Box::new(move || {
                clock.advance_ms(91_000);
                *record.lock().unwrap() = Some(collect_on(&engine, &id));
            }),
        );

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), Collected::Dropped)]);
        assert_eq!(
            *seen.lock().unwrap(),
            Some(vec![(a.clone(), Collected::Recorded)])
        );
        assert_eq!(
            state(&fx, &a).fetched_at,
            Some(NOW_S + 91),
            "the other process's reading stands"
        );
        let samples = fx
            .engine
            .store()
            .unwrap()
            .usage_samples(&a, None, 0)
            .unwrap();
        assert!(
            samples.iter().all(|(_, s)| s.fetched_at == NOW_S + 91),
            "the late result wrote no samples"
        );
        assert_eq!(usage_bearers(&fx).len(), 2);
        assert_eq!(usage_requests(&fx), 2);
    }

    #[test]
    fn a_sender_that_lost_its_lease_never_resends_a_token_refused_meanwhile() {
        // §8.3, §8.6: suspended for 91 s just before sending, past the lease. Another process
        // takes the lease over, is refused the same token (401) and stamps rejected_fp. This
        // process's copy of rejected_fp predates the stamp; the store's authorization, fenced
        // by the lease it lost, sends nothing.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(401, refused()); // the only usage reply; nothing for the token endpoint
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let seen = Arc::new(Mutex::new(None));
        let (clock, engine, id, record) =
            (fx.clock.clone(), other.clone(), a.clone(), seen.clone());
        fx.engine.on_point(
            "usage-before-send",
            Box::new(move || {
                clock.advance_ms(91_000);
                *record.lock().unwrap() = Some(collect_on(&engine, &id));
            }),
        );

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), Collected::Dropped)]);
        assert_eq!(
            *seen.lock().unwrap(),
            Some(vec![(a.clone(), failed("refresh-failed"))]),
            "the other process sent, was refused, and its gate refresh never left"
        );
        assert_eq!(
            usage_bearers(&fx),
            ["at-rt-a"],
            "the refused token went out once, from the other process"
        );
        let refused_fp = access_fp(&fx, &fx.vault_bytes(&a).unwrap());
        assert_eq!(
            state(&fx, &a).rejected_fp.as_deref(),
            Some(refused_fp.as_str())
        );
        assert_eq!(
            usage_requests(&fx),
            1,
            "only the other's sent slot: this one's never-sent slot went back"
        );
    }

    #[test]
    fn a_live_login_that_moves_mid_collection_records_nothing() {
        // By the time b's live token is read, the live login names a: that token is not b's.
        let fx = Fx::new();
        fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        fx.script_usage(200, usage_fixture());
        let config = fx.paths().global_config;
        fx.engine.on_point(
            "usage-reserved",
            Box::new(move || common::splice_oauth_account(&config, &Fx::oauth_account("a@x.co"))),
        );

        let report = fx.collect(&[&b]);

        assert_eq!(report.outcomes, [(b.clone(), Collected::Dropped)]);
        assert!(fx.http.requests().is_empty());
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
    }

    fn assert_errored(report: &tagteam_engine::collect::CollectReport, a: &AccountId, why: &str) {
        assert_eq!(report.outcomes, [(a.clone(), failed("error"))]);
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        let prefix = "usage for a@x.co (position 1) was not collected: ";
        assert!(
            report.warnings[0].starts_with(prefix) && report.warnings[0].contains(why),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn a_stamp_written_by_another_process_after_the_read_is_rejected_at_the_send() {
        // C10: this collection read its state with no `rejected_fp`; before the request,
        // another process stamps the token it is about to send. The store's authorization
        // sends nothing and the slot goes back. Inactive: an ordinary 401 failure.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        let fp = access_fp(&fx, &fx.vault_bytes(&a).unwrap());
        let path = fx.env.data_dir().join("tagteam.db");
        let id = a.clone();
        fx.engine.on_point(
            "usage-before-send",
            Box::new(move || {
                rusqlite::Connection::open(&path)
                    .unwrap()
                    .execute(
                        "INSERT INTO usage_state (account_id, rejected_fp) VALUES (?1, ?2) \
                         ON CONFLICT(account_id) DO UPDATE SET rejected_fp = excluded.rejected_fp",
                        rusqlite::params![id.as_str(), fp],
                    )
                    .unwrap();
            }),
        );

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), failed("http-401"))]);
        assert!(fx.http.requests().is_empty(), "nothing was sent");
        assert_eq!(usage_requests(&fx), 0, "the slot went back");
        assert_eq!(state(&fx, &a).consecutive_failures, 1);
    }

    #[test]
    fn an_over_budget_send_with_a_stale_slot_deletes_it_and_backs_off_past_the_free_time() {
        // C13: suspended past `slot_valid_s` between reserving and sending, while the
        // identity's other requests filled the rest of the hour. The stale slot is deleted
        // (not counted again), the failure is `over-budget`, and the backoff reaches the time
        // a slot frees.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        let clock = fx.clock.clone();
        let key = fx
            .engine
            .store()
            .unwrap()
            .account(&a)
            .unwrap()
            .unwrap()
            .identity_key;
        let path = fx.env.data_dir().join("tagteam.db");
        let provider = fx.provider();
        fx.engine.on_point(
            "usage-reserved",
            Box::new(move || {
                clock.advance_ms(61_000);
                let conn = rusqlite::Connection::open(&path).unwrap();
                for _ in 0..20 {
                    conn.execute(
                        "INSERT INTO usage_requests(provider, identity_key, at) VALUES (?1, ?2, ?3)",
                        rusqlite::params![provider.as_str(), key, NOW_S + 1],
                    )
                    .unwrap();
                }
            }),
        );

        let report = fx.collect(&[&a]);

        let free = NOW_S + 1 + 3600;
        assert_eq!(report.outcomes, [(a.clone(), failed("over-budget"))]);
        assert!(fx.http.requests().is_empty());
        assert_eq!(
            common::slot_times(&fx),
            vec![NOW_S + 1; 20],
            "the stale slot is gone and no fresh one was taken"
        );
        let s = state(&fx, &a);
        assert_eq!(s.last_error.as_deref(), Some("over-budget"));
        assert!(s.backoff_until.unwrap() >= free, "{:?}", s.backoff_until);
    }

    #[test]
    fn an_error_from_a_gate_refresh_is_a_warning_naming_the_account() {
        // C13: `refresh_stored` itself errors (here a hook at the gate's first step).
        let fx = Fx::new();
        let a = due(&fx);
        fx.engine.fail_at(Some("gate-before-request"));

        let report = fx.collect(&[&a]);
        fx.engine.fail_at(None);

        assert_eq!(report.outcomes, [(a.clone(), failed("refresh-failed"))]);
        assert_eq!(
            report.warnings,
            [
                "could not refresh a@x.co (position 1) to read its usage: injected failure at gate-before-request"
            ]
        );
        assert!(fx.http.requests().is_empty());
        assert_eq!(usage_requests(&fx), 0, "its slot went back");
    }

    #[test]
    fn an_error_after_reserving_gives_the_slot_back_and_is_the_accounts_warning() {
        // C12, C16: the hook after the reservation fails; `collect_usage` still returns.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        fx.engine.fail_at(Some("usage-reserved"));

        let report = fx.collect(&[&a]);
        fx.engine.fail_at(None);

        assert_errored(&report, &a, "injected failure at usage-reserved");
        assert!(fx.http.requests().is_empty());
        assert_eq!(usage_requests(&fx), 0, "the never-sent slot went back");
    }

    #[test]
    fn a_store_error_at_the_authorization_gives_the_held_slot_back() {
        // C16: `authorize` took the held slot before `authorize_send`, whose error lost it.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        let path = fx.env.data_dir().join("tagteam.db");
        fx.engine.on_point(
            "usage-before-send",
            Box::new(move || {
                rusqlite::Connection::open(&path)
                    .unwrap()
                    .execute_batch("ALTER TABLE leases RENAME TO leases_broken")
                    .unwrap();
            }),
        );

        let report = fx.collect(&[&a]);

        assert_errored(&report, &a, "leases");
        assert!(fx.http.requests().is_empty(), "nothing was sent");
        assert_eq!(usage_requests(&fx), 0, "the held slot went back");
    }

    #[test]
    fn an_error_before_the_record_gives_a_never_sent_slot_back() {
        // C16: the `usage-before-record` hook fails after a fetch that never left (no token).
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.kc.delete(SERVICE, a.as_str()).unwrap();
        fx.engine.fail_at(Some("usage-before-record"));

        let report = fx.collect(&[&a]);
        fx.engine.fail_at(None);

        assert_errored(&report, &a, "injected failure at usage-before-record");
        assert_eq!(usage_requests(&fx), 0, "the never-sent slot went back");
    }

    #[test]
    fn an_error_recording_a_failure_gives_its_never_sent_slot_back() {
        // C16: `record_usage_failure` itself fails (its table is gone), which would otherwise
        // leave the slot counted.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.kc.delete(SERVICE, a.as_str()).unwrap();
        let path = fx.env.data_dir().join("tagteam.db");
        fx.engine.on_point(
            "usage-before-record",
            Box::new(move || {
                rusqlite::Connection::open(&path)
                    .unwrap()
                    .execute_batch("ALTER TABLE usage_state RENAME TO usage_state_broken")
                    .unwrap();
            }),
        );

        let report = fx.collect(&[&a]);

        assert_errored(&report, &a, "usage_state");
        assert_eq!(usage_requests(&fx), 0, "the never-sent slot went back");
    }

    #[test]
    fn the_plan_is_made_for_the_role_the_account_has_when_it_records() {
        // C20: a is a candidate when its collection reads its role; a switch to a commits
        // while the fetch is in flight. The plan a records must be the active role's (180 s
        // default, grown once to 270), not the pre-switch candidate's (300, grown to 450),
        // which would overwrite the switch's re-plan for a cycle.
        let fx = Fx::new();
        let a = two_accounts(&fx); // b is live
        fx.script_usage(200, usage_fixture());
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let request = fx.switch_request(&a, false);
        fx.engine.on_point(
            "usage-before-record",
            Box::new(move || {
                other.switch(request.clone()).unwrap();
            }),
        );

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
        assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
        assert_eq!(state(&fx, &a).poll_interval_s, Some(270));
    }

    #[test]
    fn an_account_quarantined_after_the_reservation_sends_nothing_and_leaves_no_counted_slot() {
        // `authorize_send` answers `LeaseLost` for a quarantined account (C8); the held slot
        // was never sent, so the collector gives it back.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.script_usage(200, usage_fixture());
        let path = fx.env.data_dir().join("tagteam.db");
        let id = a.clone();
        fx.engine.on_point(
            "usage-before-send",
            Box::new(move || {
                rusqlite::Connection::open(&path)
                    .unwrap()
                    .execute(
                        "UPDATE accounts SET quarantine_reason = 'invalid_grant', \
                         quarantine_fp = 'sha256:x' WHERE id = ?1",
                        [id.as_str()],
                    )
                    .unwrap();
            }),
        );

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), Collected::Dropped)]);
        assert!(fx.http.requests().is_empty(), "nothing was sent");
        assert_eq!(usage_requests(&fx), 0, "the never-sent slot went back");
    }

    #[test]
    fn a_token_another_process_just_refreshed_is_still_refreshed_once_after_its_401() {
        // The gate found the vault already holding a fresh token (`AlreadyFresh`): this
        // collection has not spent its one refresh, so that token's 401 still gets it (§8.3).
        let fx = Fx::new();
        let a = due(&fx);
        fx.script_usage(401, refused());
        fx.script_refresh(Some("rt-a3"));
        fx.script_usage(200, usage_fixture());
        let (kc, id) = (fx.kc.clone(), a.clone());
        fx.engine.on_point(
            "usage-before-gate",
            Box::new(move || {
                kc.put(SERVICE, id.as_str(), &credential("a@x.co", "rt-a2"));
            }),
        );

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
        assert_eq!(usage_bearers(&fx), ["at-rt-a2", "at-rt-a3"]);
        assert_eq!(
            token_requests(&fx),
            1,
            "one refresh of this collection's own"
        );
    }

    #[test]
    fn a_stale_active_record_does_not_outrank_the_live_login() {
        // After a Claude Code `/login` outside tagteam the store's record still names b while
        // the live login names a. a's plan must use the active cadence (270), not the
        // candidate's (450), and no switch committed meanwhile.
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        fx.engine
            .store()
            .unwrap()
            .set_active(&fx.provider(), Some(&b))
            .unwrap();
        fx.login("a@x.co", "rt-a");
        fx.script_usage(200, usage_fixture());

        let report = fx.collect(&[&a]);

        assert_eq!(report.outcomes, [(a.clone(), Collected::Recorded)]);
        assert_eq!(state(&fx, &a).poll_interval_s, Some(270));
    }
}
