//! §7.2 freshen before activation, through a manual `switch`: every row of the manual-switch
//! table, quarantined targets, and when no request may be made at all.

mod common;

use std::fs;
#[cfg(feature = "test-hooks")]
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(feature = "test-hooks")]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use common::{
    API_KEY, Fx, block_rescue, crash_row, credential, journal, quarantine_of, rescue_files,
    token_requests, two_accounts, unblock_rescue, vault_fp,
};
use serde_json::json;
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::switch::SwitchOutcome;
use tagteam_engine::vault::SERVICE;
#[cfg(feature = "test-hooks")]
use tagteam_provider::Keychain;
use tagteam_provider::ProcessStamp;
use tagteam_provider::http::{HttpError, Method};

fn cannot_refresh(why: &str) -> String {
    format!("could not refresh a@x.co first ({why}); Claude Code will refresh it when it is online")
}

#[test]
fn an_expiring_target_is_refreshed_before_it_is_activated() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));
    let out = fx.switch_to(&a, false).unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
}

#[test]
fn a_signal_before_the_gate_sends_no_refresh_request() {
    // §14.1: planning, before any lock, is a cancellation point. A Ctrl-C that has landed stops
    // the switch before it spends the target's refresh token, not after.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));
    fx.engine.cancel().request(libc::SIGINT);

    let err = fx.switch_to(&a, false).unwrap_err();

    assert_eq!(err.signal(), Some(libc::SIGINT), "{err}");
    assert_eq!(token_requests(&fx), 0, "the refresh token was not spent");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn a_signal_after_the_target_is_settled_still_sends_no_refresh_request() {
    // §14.1: freshening's own cancellation point, right before the gate. The token is set on
    // the target's second vault read, the one after its account lock was taken and released
    // (§9.2's lazy capture), so no lock wait sees it: only that point can stop the switch.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));
    let cancel = fx.engine.cancel().clone();
    let key = a.as_str().to_owned();
    let reads = std::sync::atomic::AtomicUsize::new(0);
    let engine = fx.engine_with_vault_probe(move |read| {
        if read == key && reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            cancel.request(libc::SIGINT);
        }
    });

    let err = engine.switch(fx.switch_request(&a, false)).unwrap_err();

    assert!(
        matches!(err, EngineError::Interrupted(libc::SIGINT)),
        "{err:?}"
    );
    assert_eq!(token_requests(&fx), 0, "the refresh token was not spent");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn no_request_is_made_outside_the_window_for_a_self_switch_or_an_unrefreshable_kind() {
    // Outside the 10-minute window.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.switch_to(&a, false).unwrap();
    // A self-switch: a is live now, and only CC (or §7.5) refreshes the live token.
    fx.expire_access(&a);
    fx.switch_to(&a, false).unwrap();
    // An API key has no refresh token and no expiry (§7.1).
    let k = fx.add_api_key(API_KEY);
    fx.switch_to(&k, false).unwrap();
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_dead_direct_target_is_quarantined_and_refused() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_token_error(400, "invalid_grant");
    let err = fx.switch_to(&a, false).unwrap_err();
    assert_eq!(err.kind(), "relogin-required", "{err}");
    assert_eq!(
        err.to_string(),
        "a@x.co (position 1) needs a new login: its stored refresh token can no longer be used; log in with `claude`, then run `tagteam add`"
    );
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("invalid_grant"));
    assert_eq!(
        fx.live_email().as_deref(),
        Some("b@x.co"),
        "nothing activated"
    );
    assert!(journal(&fx).is_none());
    assert!(fx.displaced().is_empty());
}

#[test]
fn a_rotation_whose_pick_turns_out_dead_moves_on_to_the_next_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: c, so the rotation's pick is a (it wraps)
    fx.expire_access(&a);
    fx.script_token_error(400, "invalid_grant");
    let out = fx.engine.switch(fx.rotation_request(false)).unwrap();
    assert_eq!(out.to.as_ref().map(|r| &r.id), Some(&b));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("invalid_grant"));
    assert_eq!(token_requests(&fx), 1, "b was not in the window");
}

#[test]
fn a_busy_gate_lets_the_switch_wait_for_the_other_refresh() {
    // The cross-review's Busy path. Another process's gate holds a's account lock, spends
    // rt-a, cannot write the vault and rescues rt-a-2. This switch sends nothing, waits for
    // the lock, and activates rt-a-2.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    let held = AccountLock::acquire(&fx.env, &a, Duration::from_secs(1)).unwrap();
    let predecessor = vault_fp(&fx, &a);
    let successor = credential("a@x.co", "rt-a-2");
    let (fx_ref, a_ref) = (&fx, &a);
    let out = thread::scope(|s| {
        s.spawn(move || {
            thread::sleep(Duration::from_millis(300));
            fx_ref.plant_rescue(a_ref, &predecessor, &successor);
            drop(held);
        });
        fx.switch_to(&a, false).unwrap()
    });
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(
        token_requests(&fx),
        0,
        "single-flight: the other refresh was the one"
    );
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
}

#[test]
fn a_target_quarantined_while_the_switch_waits_is_never_activated_spent() {
    // Codex round 1: another process's gate holds a's account lock, spends rt-a, and can store
    // the successor nowhere (§7.3 `Unpersisted`), so it quarantines a `successor_lost`. This
    // switch got `Busy`, waited for the lock, and must not activate the spent rt-a: §9.4 step 1
    // re-reads the quarantine under the lock and applies §7.2's quarantined-target rule.
    for due in [true, false] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        if due {
            fx.expire_access(&a);
        }
        let held = AccountLock::acquire(&fx.env, &a, Duration::from_secs(1)).unwrap();
        let spent = vault_fp(&fx, &a);
        let (fx_ref, a_ref) = (&fx, &a);
        let result = thread::scope(|s| {
            s.spawn(move || {
                thread::sleep(Duration::from_millis(300));
                fx_ref.quarantine(a_ref, "successor_lost", &spent);
                drop(held);
            });
            fx.switch_to(&a, false)
        });
        assert_eq!(token_requests(&fx), 0, "due: {due}");
        if due {
            let err = result.unwrap_err();
            assert_eq!(err.kind(), "relogin-required", "{err}");
            assert_eq!(
                fx.live_email().as_deref(),
                Some("b@x.co"),
                "the spent rt-a is never activated"
            );
        } else {
            // Its access token still works: activated, with §7.2's warning.
            let out = result.unwrap();
            assert_eq!(
                out.warnings,
                [
                    "a@x.co (position 1) needs a new login: its stored refresh token can no longer be used; it works only until its current access token expires"
                ]
            );
            assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
        }
    }
}

#[test]
fn offline_the_switch_proceeds_with_the_vault_generation_and_a_warning() {
    // Review Focus 1, and the other transient kinds.
    let replies: [(Result<(), HttpError>, &str); 3] = [
        (
            Err(HttpError::PreSend("dns lookup failed".into())),
            "pre-send",
        ),
        (
            Err(HttpError::Ambiguous("connection reset".into())),
            "ambiguous",
        ),
        (Ok(()), "http-500"),
    ];
    for (reply, kind) in replies {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.expire_access(&a);
        let token = Fx::endpoints().token;
        match reply {
            Err(e) => fx.http.push(Method::Post, &token, Err(e)),
            Ok(()) => fx
                .http
                .push_json(Method::Post, &token, 500, json!({"error": "overloaded"})),
        }
        let out = fx.switch_to(&a, false).unwrap();
        assert_eq!(out.warnings, [cannot_refresh(kind)], "{kind}");
        assert_eq!(token_requests(&fx), 1, "{kind}");
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"), "{kind}");
        assert_eq!(
            fx.vault_refresh_token(&a).as_deref(),
            Some("rt-a"),
            "{kind}"
        );
        assert_eq!(quarantine_of(&fx, &a).0, None, "{kind}");
        assert_eq!(rescue_files(&fx), 0, "no successor was received: {kind}");
    }
}

#[test]
fn a_systemic_refusal_is_never_a_strike_and_the_switch_proceeds() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_token_error(400, "invalid_client");
    let out = fx.switch_to(&a, false).unwrap();
    assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
    assert!(out.warnings[0].starts_with("could not refresh a@x.co first ("));
    assert!(out.warnings[0].ends_with("); Claude Code will refresh it when it is online"));
    assert_eq!(quarantine_of(&fx, &a).0, None);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
}

#[test]
fn a_rescued_successor_refuses_the_switch_until_the_vault_takes_it() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));
    fx.kc.set_fail_write(SERVICE, true);
    let err = fx.switch_to(&a, false).unwrap_err();
    fx.kc.set_fail_write(SERVICE, false);
    assert_eq!(err.kind(), "rescue-pending", "{err}");
    assert_eq!(
        fx.live_email().as_deref(),
        Some("b@x.co"),
        "the spent rt-a is never activated"
    );
    assert_eq!(rescue_files(&fx), 1);
    // Next time, the gate adopts the rescue (§7.3 step 3) and needs no request.
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
    assert_eq!(token_requests(&fx), 1);
}

#[test]
fn an_unpersisted_successor_refuses_the_switch_and_asks_for_a_new_login() {
    // The successor is lost and the vault's generation is spent (§7.3 step 6): retrying
    // cannot help, and activating would hand CC a used refresh token.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));
    fx.kc.set_fail_write(SERVICE, true);
    // `rescue/` lists fine but cannot be written to (0500), so the gate still sends the
    // request, then has nowhere to keep the successor.
    block_rescue(&fx);
    let err = fx.switch_to(&a, false).unwrap_err();
    fx.kc.set_fail_write(SERVICE, false);
    unblock_rescue(&fx);
    assert_eq!(err.kind(), "relogin-required", "{err}");
    assert!(err.to_string().contains("can no longer be used"), "{err}");
    assert_eq!(token_requests(&fx), 1, "the request was sent");
    assert_eq!(
        quarantine_of(&fx, &a).0.as_deref(),
        Some("successor_lost"),
        "the spent generation is quarantined (§7.4)"
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn an_unreadable_rescue_refuses_without_a_request_and_names_the_file() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    let path = fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    fs::write(&path, b"{\"format\":\"tagteam-res").unwrap();
    let err = fx.switch_to(&a, false).unwrap_err();
    assert_eq!(err.kind(), "rescue-pending", "{err}");
    assert!(
        err.to_string()
            .contains(path.file_name().unwrap().to_str().unwrap()),
        "{err}"
    );
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn a_rescue_the_vault_cannot_adopt_refuses_with_a_detail_that_says_so() {
    // Freshening settles the target's rescues under its account lock before the gate runs
    // (§6.2), so the refusal is the settle's own, naming the file it could not adopt. No file
    // is unreadable, so it must not name an empty list of unreadable ones.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    fx.kc.set_fail_write(SERVICE, true);
    let err = fx.switch_to(&a, false).unwrap_err();
    fx.kc.set_fail_write(SERVICE, false);
    assert_eq!(err.kind(), "rescue-pending", "{err}");
    let shown = err.to_string();
    assert!(shown.contains("could not be adopted"), "{shown}");
    assert!(
        shown.ends_with("retry once the vault can be written"),
        "{shown}"
    );
    assert!(!shown.contains(" cannot be read"), "{shown}");
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    // Once the vault can be written, the next switch adopts the rescue and activates it.
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_quarantined_target_works_until_its_access_token_expires() {
    // Outside the window: activated, with a warning.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
    let out = fx.switch_to(&a, false).unwrap();
    assert_eq!(
        out.warnings,
        [
            "a@x.co (position 1) needs a new login: its stored refresh token can no longer be used; it works only until its current access token expires"
        ]
    );
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));

    // Inside the window: refused, and never refreshed (§7.4).
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
    fx.expire_access(&a);
    let err = fx.switch_to(&a, false).unwrap_err();
    assert_eq!(err.kind(), "relogin-required", "{err}");
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn a_strike_on_the_live_account_after_planning_holds_while_its_live_credential_is_bound() {
    // Another process's gate quarantines b, the live account, after this forced self-switch
    // has planned on b's unquarantined row and before this switch's own gate runs. b's vault
    // has moved on, but the live credential is still the generation the strike is bound to
    // (§7.4), so the gate must not release it: it reports Dead, and the direct switch refuses
    // (§7.2) instead of activating the vault's generation.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: rt-b
    let bound = vault_fp(&fx, &b);
    fx.put_vault(&b, &credential("b@x.co", "rt-b2"));
    fx.expire_access(&b); // due: the forced self-switch freshens the vault's generation
    let strike = Mutex::new(Some((fx.engine.store().unwrap(), b.clone(), bound.clone())));
    let key = b.to_string();
    // `plan` reads b's vault first (`has_login`): the strike lands there, once.
    let engine = fx.engine_with_vault_probe(move |read| {
        if read != key {
            return;
        }
        if let Some((store, id, fp)) = strike.lock().unwrap().take() {
            store.set_quarantine(&id, "invalid_grant", &fp, 1).unwrap();
        }
    });
    let err = engine.switch(fx.switch_request(&b, true)).unwrap_err();
    assert_eq!(err.kind(), "relogin-required", "{err}");
    assert_eq!(
        quarantine_of(&fx, &b),
        (Some("invalid_grant".into()), Some(bound))
    );
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_forced_switch_still_freshens_its_target() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("stranger@x.co", "rt-s"); // unmanaged: --force takes the direct branch
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));
    fx.switch_to(&a, true).unwrap();
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-2"));
}

#[test]
fn a_forced_self_switch_follows_the_quarantined_target_rule() {
    // `--force` on the live account re-activates the vault's generation (§9.2), not the live
    // one, so §7.2's quarantined-target rule applies to it as to any other target.
    let works = "a@x.co (position 1) needs a new login: its stored refresh token can no longer be used; it works only until its current access token expires";
    for due in [true, false] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        fx.switch_to(&a, false).unwrap();
        fx.quarantine(&a, "successor_lost", &vault_fp(&fx, &a));
        fx.login("a@x.co", "rt-a-new"); // a newer login tagteam never stored
        if due {
            fx.expire_access(&a);
            let err = fx.switch_to(&a, true).unwrap_err();
            assert_eq!(err.kind(), "relogin-required", "{err}");
            assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a-new"));
        } else {
            let out = fx.switch_to(&a, true).unwrap();
            assert!(
                out.warnings.iter().any(|w| w == works),
                "{:?}",
                out.warnings
            );
        }
        assert_eq!(token_requests(&fx), 0);
    }

    // Not quarantined and due: the gate leaves the live account's refresh to CC (§7.3 step 2).
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.switch_to(&a, false).unwrap();
    fx.expire_access(&a);
    fx.switch_to(&a, true).unwrap();
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
}

/// Runs a switch to `a` on an engine that stops at its first read of `a`'s vault after
/// planning began (a read in `plan`, before any freshening), and runs `act` on the test thread
/// while it is stopped. The engine shares this fixture's Env, Keychain and HTTP port, so `act`
/// changes what the switch goes on to find.
/// `act` may hand back a thread it started, which is joined once the switch is done.
fn switch_after(
    fx: &Fx,
    a: &AccountId,
    act: impl FnOnce(&Fx) -> Option<thread::JoinHandle<()>>,
) -> Result<SwitchOutcome, EngineError> {
    let (paused_tx, paused_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let waiting = Mutex::new(Some((paused_tx, go_rx)));
    let key = a.to_string();
    let engine = fx.engine_with_vault_probe(move |read| {
        let taken = waiting.lock().unwrap().take();
        if let (true, Some((paused, go))) = (read.starts_with(&key), taken) {
            paused.send(()).unwrap();
            go.recv().unwrap();
        }
    });
    thread::scope(|s| {
        let switching = s.spawn(|| engine.switch(fx.switch_request(a, false)));
        paused_rx.recv().unwrap();
        let started = act(fx);
        go_tx.send(()).unwrap();
        let outcome = switching.join().unwrap();
        if let Some(t) = started {
            t.join().unwrap();
        }
        outcome
    })
}

#[cfg(feature = "test-hooks")]
#[test]
fn a_quarantine_the_plan_already_saw_is_still_applied_under_the_locks() {
    // Attempt 1 plans again for an unrelated reason (the vault reads empty for a moment) and
    // releases the account locks. Another process's gate then quarantines a `successor_lost`,
    // so attempt 2's plan sees the quarantine itself. Nothing but `rederive` applies §7.2's
    // rule under the locks, so it must not skip a quarantine the plan saw.
    for due in [true, false] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        if due {
            fx.expire_access(&a);
            // Offline when freshening: the switch goes on with the vault's generation.
            let token = Fx::endpoints().token;
            let refused = HttpError::PreSend("dns lookup failed".into());
            fx.http.push(Method::Post, &token, Err(refused));
        }
        let saved = fx.vault_bytes(&a).unwrap();
        let spent = vault_fp(&fx, &a);
        let store = fx.engine.store().unwrap();
        let step = Arc::new(AtomicUsize::new(0));
        let (kc, key) = (fx.kc.clone(), a.to_string());
        let seen = step.clone();
        let engine = fx.engine_with_vault_probe({
            let (kc, key, store, a) = (kc.clone(), key.clone(), store.clone(), a.clone());
            let step = step.clone();
            move |read| {
                if !read.starts_with(&key) {
                    return;
                }
                match step.load(Ordering::SeqCst) {
                    // Attempt 1's locked re-read: empty, and quarantined meanwhile.
                    1 => {
                        kc.delete(SERVICE, &key).unwrap();
                        store
                            .set_quarantine(&a, "successor_lost", &spent, 1)
                            .unwrap();
                        step.store(2, Ordering::SeqCst);
                    }
                    // Attempt 2's plan: the vault is back (rt-a, spent as far as the row says).
                    2 => {
                        kc.put(SERVICE, &key, &saved);
                        step.store(3, Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
        });
        engine.on_point("planned", Box::new(move || seen.store(1, Ordering::SeqCst)));
        let result = engine.switch(fx.switch_request(&a, false));
        assert_eq!(
            step.load(Ordering::SeqCst),
            3,
            "due: {due}: {:?}",
            result.as_ref().map(|o| o.reason.as_str())
        );
        assert_eq!(
            token_requests(&fx),
            usize::from(due),
            "only freshening sends"
        );
        if due {
            let err = result.unwrap_err();
            assert_eq!(err.kind(), "relogin-required", "{err}");
            assert_eq!(
                fx.live_email().as_deref(),
                Some("b@x.co"),
                "the spent rt-a is never activated"
            );
        } else {
            let out = result.unwrap();
            assert_eq!(
                out.warnings,
                [
                    "a@x.co (position 1) needs a new login: its stored refresh token can no longer be used; it works only until its current access token expires"
                ]
            );
            assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
        }
    }
}

#[test]
fn a_journal_row_naming_the_target_is_left_to_the_mutation_lock() {
    // The gate cannot tell a switch still in progress from an interrupted one. Another
    // process is mid-switch to a: it holds the mutation lock and its live journal row names
    // a. This switch must not refuse with `interrupted-switch` from the freshen step; it
    // warns, waits for the mutation lock, and finds the row gone.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.expire_access(&a);
    let mut row = crash_row(&fx, &b, &a);
    row.holder = ProcessStamp::current().unwrap();
    let out = switch_after(&fx, &a, |fx| {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (row, other) = (row.clone(), fx.engine_with_env(fx.env.clone()));
        let holder = thread::spawn(move || {
            let guard = other.mutation_guard().unwrap();
            let store = other.store().unwrap();
            store.insert_journal(&row).unwrap();
            ready_tx.send(()).unwrap();
            thread::sleep(Duration::from_millis(300));
            store.delete_journal(&row.provider).unwrap();
            drop(guard);
        });
        ready_rx.recv().unwrap();
        Some(holder)
    })
    .unwrap();
    assert_eq!(
        out.warnings,
        [cannot_refresh("an unfinished switch names it")]
    );
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
    assert!(journal(&fx).is_none());
}

#[test]
fn a_target_the_gate_finds_live_is_left_alone_with_one_warning() {
    // The live identity flakes between planning and the gate: the gate finds the target live
    // (§7.3 step 2), never refreshes it, and the switch plans again as a self-switch. The
    // warning is carried into that outcome.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    let out = switch_after(&fx, &a, |fx| {
        fx.login("a@x.co", "rt-a");
        None
    })
    .unwrap();
    assert!(!out.switched, "{out:?}");
    assert_eq!(out.reason.as_str(), "already-active");
    assert_eq!(out.warnings, [cannot_refresh("it may be the live login")]);
    assert_eq!(token_requests(&fx), 0);
}
