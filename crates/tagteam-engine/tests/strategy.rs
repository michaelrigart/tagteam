//! §9.3's usage strategies, `best` and `next-available`: the release of quarantines that no
//! longer bind (§7.4, Decision 9), on-demand collection (§8.3, Decision 8), ranking from
//! decision-grade readings (§8.4), lazy vault reads, freshening (§7.2), and what stands under
//! the locks (Decision 11, Review Focus 4). Readings are recorded through the store's own
//! reserve-and-record calls, as the collector records them.
mod common;

use std::fs;

use common::{
    API_KEY, Fx, OTHER_API_KEY, credential, quarantine_of, usage_bearers, usage_fixture, vault_fp,
};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, PollBudget, PollPlan, Window, WindowKind};
use tagteam_engine::EngineError;
use tagteam_engine::settings::Settings;
use tagteam_engine::store::Reserve;
use tagteam_engine::switch::{
    SwitchOutcome, SwitchReason, SwitchRequest, SwitchTarget, UsageStrategy,
};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::Keychain;

#[test]
fn nothing_is_released_or_created_without_a_store() {
    let fx = Fx::new();
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert!(!fx.env.data_dir().join("tagteam.db").exists());
}

#[test]
fn a_quarantine_the_vault_has_moved_past_is_released_and_recorded_with_its_source() {
    // M3b's tick passes "auto"; the event says who released it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    assert_eq!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "auto")
            .unwrap(),
        [a.clone()]
    );
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    let events = fx.engine.store().unwrap().events().unwrap();
    let last = events.last().unwrap();
    assert_eq!(
        (
            last.kind.as_str(),
            last.to_id.as_ref(),
            last.source.as_str()
        ),
        ("unquarantine", Some(&a), "auto")
    );
}

#[test]
fn a_quarantine_the_vault_still_holds_stays() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let bound = vault_fp(&fx, &a);
    fx.quarantine(&a, "invalid_grant", &bound);
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("invalid_grant".into()), Some(bound))
    );
}

#[test]
fn an_unreadable_vault_leaves_the_quarantine() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    fx.kc.set_unreadable(SERVICE, a.as_str(), true);
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert!(quarantine_of(&fx, &a).0.is_some());
}

#[test]
fn the_live_account_s_quarantine_holds_while_the_live_credential_is_its_generation() {
    // §7.4: the active account's quarantine holds while either copy matches `quarantine_fp`.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: rt-b
    let bound = vault_fp(&fx, &b);
    fx.quarantine(&b, "invalid_grant", &bound);
    // The vault moves on; the live credential is still the generation the strike is bound to.
    fx.put_vault(&b, &credential("b@x.co", "rt-b2"));
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert_eq!(quarantine_of(&fx, &b).1, Some(bound));
    // Claude Code rotates the live credential too: neither copy is bound any more.
    fx.rotate_live("rt-b3");
    assert_eq!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap(),
        [b.clone()]
    );
    assert_eq!(quarantine_of(&fx, &b), (None, None));
}

#[test]
fn a_busy_account_lock_leaves_the_quarantine_for_the_next_caller() {
    // Decision 9: try-only. Whoever holds the lock may be writing this very account.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    let held = fx.engine.lock_account(&a).unwrap();
    assert!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
            .is_empty()
    );
    assert!(quarantine_of(&fx, &a).0.is_some());
    drop(held);
    assert_eq!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap(),
        [a]
    );
}

/// The fixture clock's start (`Fx`), in seconds.
const T0: i64 = 1_790_000_000;

fn window(key: &str, label: &str, kind: WindowKind, pct: f64, resets_at: i64) -> Window {
    Window {
        key: key.into(),
        label: label.into(),
        kind,
        pct,
        resets_at: Some(resets_at),
        period_s: match kind {
            WindowKind::Short => Some(18_000),
            _ => Some(604_800),
        },
        detail: None,
    }
}

/// A Claude Code reading: 5h at `five` (resetting at T0 + 2h40m30s), 7d at `seven` and the
/// Fable model window at `fable` (both resetting at T0 + 3d09h00m30s).
fn reading(five: f64, seven: f64, fable: f64) -> Vec<Window> {
    vec![
        window("5h", "5h", WindowKind::Short, five, T0 + 9_630),
        window("7d", "7d", WindowKind::Long, seven, T0 + 291_630),
        window(
            "scoped:Fable",
            "Fable",
            WindowKind::Scoped,
            fable,
            T0 + 291_630,
        ),
    ]
}

/// Records `windows` as `id`'s reading taken at `at`, its next poll planned at `next_poll_at`,
/// through the collector's own reserve (§8.3 phase 1) and record (phase 3).
fn record_at(fx: &Fx, id: &AccountId, windows: &[Window], at: i64, next_poll_at: i64) {
    let store = fx.engine.store().unwrap();
    let row = store.account(id).unwrap().unwrap();
    let r = match store
        .reserve_usage(&row, at * 1000, false, &PollBudget::STANDARD)
        .unwrap()
    {
        Reserve::Reserved(r) => r,
        other => panic!("not reserved at {at}: {other:?}"),
    };
    let plan = PollPlan {
        interval_s: next_poll_at - at,
        next_poll_at,
    };
    assert!(store.record_usage(&r, windows, at, &plan, 180).unwrap());
}

/// A reading taken now with a plan in force: decision-grade, and not due on demand, so the
/// strategy's collection sends nothing for it.
fn read(fx: &Fx, id: &AccountId, windows: &[Window]) {
    record_at(fx, id, windows, T0, T0 + 300);
}

fn usage(strategy: UsageStrategy, models: Option<Vec<&str>>) -> SwitchTarget {
    SwitchTarget::Usage {
        strategy,
        models: models.map(|m| m.into_iter().map(str::to_owned).collect()),
    }
}

fn request(fx: &Fx, target: SwitchTarget) -> SwitchRequest {
    SwitchRequest {
        provider: fx.provider(),
        target,
        force: false,
        source: "cli",
    }
}

fn best(fx: &Fx) -> Result<SwitchOutcome, EngineError> {
    fx.engine
        .switch(request(fx, usage(UsageStrategy::Best, None)))
}

fn next_available(fx: &Fx) -> Result<SwitchOutcome, EngineError> {
    fx.engine
        .switch(request(fx, usage(UsageStrategy::NextAvailable, None)))
}

/// `a`, `b` and `c` at positions 1, 2 and 3; `c` is live.
fn three(fx: &Fx) -> (AccountId, AccountId, AccountId) {
    (
        fx.add("a@x.co", "rt-a"),
        fx.add("b@x.co", "rt-b"),
        fx.add("c@x.co", "rt-c"),
    )
}

/// `claude /logout`: no `oauthAccount` and no credential. The store is not told.
fn log_out(fx: &Fx) {
    fs::write(fx.paths().global_config, common::CLAUDE_JSON).unwrap();
    fx.kc
        .delete(
            &keychain_service(&fx.env, ItemKind::OAuth),
            &keychain_account(&fx.env),
        )
        .unwrap();
}

#[test]
fn reasons_and_strategies_carry_the_spec_s_tokens() {
    assert_eq!(UsageStrategy::Best.as_str(), "best");
    assert_eq!(UsageStrategy::NextAvailable.as_str(), "next-available");
    assert_eq!(SwitchReason::UsageUnavailable.as_str(), "usage-unavailable");
    assert_eq!(SwitchReason::AlreadyBest.as_str(), "already-best");
    assert_eq!(
        SwitchReason::CandidatesExhausted.as_str(),
        "candidates-exhausted"
    );
}

#[test]
fn best_switches_to_the_candidate_with_the_most_headroom() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 50.0, 0.0));
    read(&fx, &b, &reading(10.0, 20.0, 0.0));
    read(&fx, &c, &reading(10.0, 90.0, 0.0));
    let out = best(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (true, SwitchReason::Switched, "best")
    );
    assert_eq!(out.to.unwrap().id, b);
    assert_eq!(out.from.unwrap().id, c);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert!(
        usage_bearers(&fx).is_empty(),
        "every reading was fresh: nothing was fetched"
    );
}

#[test]
fn best_stays_when_no_candidate_beats_the_live_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx, &a, &reading(10.0, 60.0, 0.0));
    read(&fx, &b, &reading(10.0, 60.0, 0.0));
    let out = best(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (false, SwitchReason::AlreadyBest, "best")
    );
    assert_eq!(
        out.message,
        "b@x.co already has the most headroom (7d at 60%); the best candidate is a@x.co (7d at 60%)"
    );
    assert_eq!(out.from.map(|r| r.id), Some(b));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn best_without_a_known_candidate_is_usage_unavailable() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a"); // never read
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx, &b, &reading(10.0, 60.0, 0.0));
    let out = best(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (false, SwitchReason::UsageUnavailable, "best")
    );
    assert_eq!(
        out.message,
        "no candidate has a usage reading recent enough to rank by"
    );
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    // a was collected first (Decision 8); the scripted port has no reply, so it stays unknown.
    assert_eq!(usage_bearers(&fx), ["at-rt-a"]);
}

#[test]
fn best_counts_the_candidates_it_could_not_rank_in_a_warning() {
    let fx = Fx::new();
    let (a, _b, c) = three(&fx); // b is never read
    read(&fx, &a, &reading(10.0, 20.0, 0.0));
    read(&fx, &c, &reading(10.0, 60.0, 0.0));
    let out = best(&fx).unwrap();
    assert_eq!(out.to.unwrap().id, a);
    assert_eq!(
        out.warnings,
        ["1 candidate has no usage reading recent enough to rank by; it was not considered"]
    );
}

#[test]
fn best_with_the_live_usage_unknown_switches_to_the_best_known_with_a_warning() {
    let fx = Fx::new();
    let (a, b, _c) = three(&fx); // c is live and never read
    read(&fx, &a, &reading(10.0, 50.0, 0.0));
    read(&fx, &b, &reading(10.0, 20.0, 0.0));
    let out = best(&fx).unwrap();
    assert_eq!((out.switched, out.to.unwrap().id), (true, b));
    assert_eq!(
        out.warnings,
        ["switching to the best known candidate; the live account's usage is unknown"]
    );
}

#[test]
fn best_with_the_live_usage_unknown_never_picks_a_candidate_known_to_be_at_its_limit() {
    // §9.3: a managed live login of unknown usage may be switched away from, but never to an
    // account known to be exhausted. Here a is exhausted and b is not: b is the pick.
    let fx = Fx::new();
    let (a, b, _c) = three(&fx); // c is live and never read
    read(&fx, &a, &reading(100.0, 20.0, 0.0));
    read(&fx, &b, &reading(10.0, 95.0, 0.0));
    let out = best(&fx).unwrap();
    assert_eq!((out.switched, out.to.unwrap().id), (true, b));
    assert_eq!(
        out.warnings,
        ["switching to the best known candidate; the live account's usage is unknown"]
    );
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn best_with_the_live_usage_unknown_and_every_known_candidate_exhausted_switches_nowhere() {
    let fx = Fx::new();
    let (a, b, _c) = three(&fx); // c is live and never read
    read(&fx, &a, &reading(10.0, 100.0, 0.0));
    read(&fx, &b, &reading(104.0, 100.0, 0.0));
    let out = best(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (false, SwitchReason::CandidatesExhausted, "best")
    );
    assert_eq!(
        out.message,
        "every candidate is at its limit: a@x.co (7d at 100%), b@x.co (5h at 104%); the earliest reset is in 3d09h"
    );
    assert_eq!(
        fx.live_email().as_deref(),
        Some("c@x.co"),
        "nothing switched"
    );
}

#[test]
fn best_with_no_live_login_still_switches_to_an_exhausted_candidate_with_a_warning() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read(&fx, &a, &reading(10.0, 100.0, 0.0));
    read(&fx, &b, &reading(104.0, 100.0, 0.0));
    log_out(&fx);
    let out = best(&fx).unwrap();
    assert_eq!((out.switched, out.reason), (true, SwitchReason::Switched));
    assert_eq!(out.to.unwrap().id, a, "the most headroom of those known");
    assert_eq!(
        out.warnings,
        ["switching to the best known candidate; there is no managed live login to compare with"]
    );
}

#[test]
fn best_with_no_live_login_switches_to_the_best_known_with_a_warning() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    read(&fx, &a, &reading(10.0, 20.0, 0.0));
    read(&fx, &b, &reading(10.0, 50.0, 0.0));
    log_out(&fx);
    let out = best(&fx).unwrap();
    assert_eq!((out.switched, out.reason), (true, SwitchReason::Switched));
    assert_eq!(out.to.unwrap().id, a);
    assert_eq!(
        out.warnings,
        ["switching to the best known candidate; there is no managed live login to compare with"]
    );
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}

#[test]
fn a_better_candidate_without_a_stored_credential_leaves_the_live_account_the_best() {
    // §9.3: a switchable account has a vault credential. The walk reads only the candidates
    // that beat the live account, finds none it can activate, and so none beats it.
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 10.0, 0.0));
    read(&fx, &b, &reading(10.0, 50.0, 0.0));
    read(&fx, &c, &reading(10.0, 30.0, 0.0));
    fx.kc.delete(SERVICE, a.as_str()).unwrap();
    let out = best(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason),
        (false, SwitchReason::AlreadyBest)
    );
    assert_eq!(
        out.message,
        "no candidate with more headroom than c@x.co (7d at 30%) holds a stored credential"
    );
}

#[test]
fn next_available_skips_exhausted_candidates_and_names_their_binding_windows() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx); // c is live: the walk is a, b
    read(&fx, &a, &reading(100.0, 40.0, 0.0));
    read(&fx, &b, &reading(10.0, 95.0, 0.0));
    read(&fx, &c, &reading(10.0, 50.0, 0.0));
    let out = next_available(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (true, SwitchReason::Switched, "next-available")
    );
    assert_eq!(out.to.unwrap().id, b, "5 points left is not at the limit");
    assert_eq!(
        out.warnings,
        ["skipped a@x.co (position 1): at its limit (5h at 100%)"]
    );
}

#[test]
fn next_available_never_skips_an_unknown_candidate() {
    let fx = Fx::new();
    let (a, b, _c) = three(&fx); // a is never read
    read(&fx, &b, &reading(10.0, 20.0, 0.0));
    let out = next_available(&fx).unwrap();
    assert_eq!(out.to.unwrap().id, a);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
}

#[test]
fn next_available_with_every_candidate_exhausted_names_each_and_the_earliest_reset() {
    // A candidate is back only once every window at its limit has reset: b is at its limit in
    // both the 5h window (resets in 2h40m) and the 7d one (3d09h), so b is back in 3d09h, not
    // 2h40m. a is at its limit in the 7d window alone. The earliest candidate is back in 3d09h.
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 100.0, 0.0));
    read(&fx, &b, &reading(104.0, 100.0, 0.0));
    read(&fx, &c, &reading(10.0, 50.0, 0.0));
    let out = next_available(&fx).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.strategy),
        (false, SwitchReason::CandidatesExhausted, "next-available")
    );
    assert_eq!(
        out.message,
        "every candidate is at its limit: a@x.co (7d at 100%), b@x.co (5h at 104%); the earliest reset is in 3d09h"
    );
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
}

#[test]
fn the_earliest_reset_is_when_the_first_candidate_is_back_not_when_a_window_resets() {
    // a is blocked by its 5h window alone, which resets in 2h40m; b by both its windows, so
    // not for 3d09h. The first one back is a, in 2h40m.
    let fx = Fx::new();
    let (a, b, _c) = three(&fx);
    read(&fx, &a, &reading(100.0, 20.0, 0.0));
    read(&fx, &b, &reading(100.0, 100.0, 0.0));
    let out = next_available(&fx).unwrap();
    assert_eq!(out.reason, SwitchReason::CandidatesExhausted);
    assert!(
        out.message.ends_with("; the earliest reset is in 2h40m"),
        "{}",
        out.message
    );
}

#[test]
fn fewer_than_two_candidates_is_only_one_account_for_either_strategy() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    read(&fx, &a, &reading(10.0, 50.0, 0.0));
    for strategy in [UsageStrategy::Best, UsageStrategy::NextAvailable] {
        let out = fx
            .engine
            .switch(request(&fx, usage(strategy, None)))
            .unwrap();
        assert_eq!(
            (out.switched, out.reason, out.strategy),
            (false, SwitchReason::OnlyOneAccount, strategy.as_str())
        );
    }
}

#[test]
fn an_unreadable_vault_before_the_pick_names_the_account() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 10.0, 0.0));
    read(&fx, &b, &reading(10.0, 20.0, 0.0));
    read(&fx, &c, &reading(10.0, 90.0, 0.0));
    fx.kc.set_unreadable(SERVICE, a.as_str(), true);
    let err = best(&fx).unwrap_err();
    assert!(
        matches!(&err, EngineError::UnreadableAccount { position: 1, label, .. } if label == "a@x.co"),
        "{err}"
    );
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
}

#[test]
fn a_quarantine_that_no_longer_binds_is_released_before_candidates_are_counted() {
    // With a still quarantined, b would be the only candidate: only-one-account.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    read(&fx, &a, &reading(10.0, 10.0, 0.0));
    read(&fx, &b, &reading(10.0, 60.0, 0.0));
    fx.quarantine(&a, "invalid_grant", "sha256:a-generation-long-gone");
    let out = best(&fx).unwrap();
    assert_eq!(out.to.map(|r| r.id), Some(a.clone()), "{}", out.message);
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    // Recorded with the switch's own source.
    let events = fx.engine.store().unwrap().events().unwrap();
    let released = events.iter().find(|e| e.kind == "unquarantine").unwrap();
    assert_eq!(
        (released.to_id.as_ref(), released.source.as_str()),
        (Some(&a), "cli")
    );
}

#[test]
fn a_quarantine_still_bound_keeps_the_account_out() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 10.0, 0.0));
    read(&fx, &b, &reading(10.0, 50.0, 0.0));
    read(&fx, &c, &reading(10.0, 90.0, 0.0));
    let bound = vault_fp(&fx, &a);
    fx.quarantine(&a, "invalid_grant", &bound);
    let out = best(&fx).unwrap();
    assert_eq!(out.to.unwrap().id, b);
    assert_eq!(
        quarantine_of(&fx, &a),
        (Some("invalid_grant".into()), Some(bound))
    );
}

#[test]
fn a_dead_pick_is_quarantined_and_the_strategy_plans_again_without_it() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 10.0, 0.0));
    read(&fx, &b, &reading(10.0, 50.0, 0.0));
    read(&fx, &c, &reading(10.0, 90.0, 0.0));
    fx.expire_access(&a); // due for freshening (§7.2)
    fx.script_token_error(400, "invalid_grant");
    let out = best(&fx).unwrap();
    assert_eq!(out.to.map(|r| r.id), Some(b));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert_eq!(quarantine_of(&fx, &a).0.as_deref(), Some("invalid_grant"));
}

#[test]
fn collection_fetches_only_what_the_on_demand_rule_allows() {
    // §8.3: on demand, a reading must be older than 180 s, and due, to be fetched again.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    record_at(&fx, &a, &reading(10.0, 10.0, 0.0), T0 - 100, T0 - 100);
    record_at(&fx, &b, &reading(10.0, 10.0, 0.0), T0 - 200, T0 - 200);
    fx.script_usage(200, usage_fixture()); // b's fetch: 7d at 77 %
    let out = best(&fx).unwrap();
    assert_eq!(
        usage_bearers(&fx),
        ["at-rt-b"],
        "a's reading is 100 s old: not fetched"
    );
    assert_eq!(fx.usage_state(&a).unwrap().fetched_at, Some(T0 - 100));
    assert_eq!(fx.usage_state(&b).unwrap().fetched_at, Some(T0));
    assert_eq!(
        out.to.map(|r| r.id),
        Some(a),
        "ranked on b's new reading, 23 points left, against a's 90; b's old one tied"
    );
}

#[test]
fn model_names_decide_which_scoped_windows_count() {
    // a: 7d at 20 but Fable at 95; b: 7d at 30, Fable unused; c is live at 7d 90.
    let pick = |settings: Settings, models: Option<Vec<&str>>| {
        let fx = Fx::new();
        let (a, b, c) = three(&fx);
        read(&fx, &a, &reading(10.0, 20.0, 95.0));
        read(&fx, &b, &reading(10.0, 30.0, 0.0));
        read(&fx, &c, &reading(10.0, 90.0, 0.0));
        let engine = fx.engine_with_settings(settings);
        let to = engine
            .switch(request(&fx, usage(UsageStrategy::Best, models)))
            .unwrap()
            .to
            .unwrap()
            .id;
        if to == a {
            "a"
        } else if to == b {
            "b"
        } else {
            "?"
        }
    };
    let fable = Settings {
        models: vec!["Fable".into()],
        ..Settings::default()
    };
    assert_eq!(
        pick(Settings::default(), None),
        "a",
        "no model window counts by default"
    );
    assert_eq!(
        pick(Settings::default(), Some(vec!["fable"])),
        "b",
        "--model names it, in any case"
    );
    assert_eq!(pick(Settings::default(), Some(vec!["all"])), "b");
    assert_eq!(
        pick(Settings::default(), Some(vec!["opus"])),
        "a",
        "a model window counts only when named"
    );
    assert_eq!(
        pick(fable.clone(), None),
        "b",
        "autoswitch.models applies without --model"
    );
    assert_eq!(
        pick(fable, Some(vec![])),
        "a",
        "--model overrides it for this switch"
    );
}

/// Review Focus 4: a hotkey pressed twice. The other process lands c → a while this one waits
/// for the mutation lock; this one finds its pick already live and stops there. Planning again
/// from a would move next-available on to b, a third account.
#[cfg(feature = "test-hooks")]
#[test]
fn a_double_fired_strategy_switches_once() {
    for strategy in [UsageStrategy::Best, UsageStrategy::NextAvailable] {
        let fx = Fx::new();
        let (a, b, c) = three(&fx);
        read(&fx, &a, &reading(10.0, 10.0, 0.0));
        read(&fx, &b, &reading(10.0, 50.0, 0.0));
        read(&fx, &c, &reading(10.0, 90.0, 0.0));
        let other = fx.engine_with_env(fx.env.clone());
        let req = request(&fx, usage(strategy, None));
        let first = req.clone();
        fx.engine.on_point(
            "planned",
            Box::new(move || assert!(other.switch(first.clone()).unwrap().switched)),
        );
        let out = fx.engine.switch(req).unwrap();
        assert_eq!(
            (out.switched, out.reason, out.strategy),
            (false, SwitchReason::AlreadyActive, strategy.as_str())
        );
        assert_eq!(out.from.map(|r| r.id), Some(a.clone()));
        assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
        let switches = fx
            .engine
            .store()
            .unwrap()
            .events()
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "switch")
            .count();
        assert_eq!(switches, 1, "{strategy:?}");
    }
}

/// Decision 11: under the locks the ranking is not recomputed. The pick, a, is disabled by
/// another process while this one waits for the mutation lock; planning again from the
/// store's readings lands on b, and nothing more is fetched.
#[cfg(feature = "test-hooks")]
#[test]
fn a_pick_that_stops_being_a_candidate_while_the_switch_waits_is_replaced_from_the_store() {
    let fx = Fx::new();
    let (a, b, c) = three(&fx);
    read(&fx, &a, &reading(10.0, 10.0, 0.0));
    read(&fx, &b, &reading(10.0, 50.0, 0.0));
    read(&fx, &c, &reading(10.0, 90.0, 0.0));
    let other = fx.engine_with_env(fx.env.clone());
    let id = a.clone();
    fx.engine.on_point(
        "planned",
        Box::new(move || {
            other.set_disabled(&id, true).unwrap();
        }),
    );
    let out = best(&fx).unwrap();
    assert_eq!((out.switched, out.to.map(|r| r.id)), (true, Some(b)));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert!(usage_bearers(&fx).is_empty());
}

#[test]
fn a_live_credential_with_no_token_to_compare_holds_the_live_quarantine() {
    // An empty live read, as a Keychain timeout can look, or a torn credential, may be exactly
    // the bound generation (§7.4): only an absent one, or one carrying another, lets go.
    for unreadable in [&b""[..], b"{\"claudeAiOauth\": "] {
        let fx = Fx::new();
        fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b"); // live: rt-b
        let bound = vault_fp(&fx, &b);
        fx.quarantine(&b, "invalid_grant", &bound);
        fx.put_vault(&b, &credential("b@x.co", "rt-b2"));
        fx.set_live_credential(unreadable);
        assert!(
            fx.engine
                .release_unbound_quarantines(&fx.provider(), "cli")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            quarantine_of(&fx, &b),
            (Some("invalid_grant".into()), Some(bound))
        );
    }
}

#[test]
fn a_live_account_with_no_live_credential_has_nothing_the_quarantine_binds() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live: rt-b
    fx.quarantine(&b, "invalid_grant", &vault_fp(&fx, &b));
    fx.put_vault(&b, &credential("b@x.co", "rt-b2"));
    fx.kc
        .delete(
            &keychain_service(&fx.env, ItemKind::OAuth),
            &keychain_account(&fx.env),
        )
        .unwrap();
    assert_eq!(
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap(),
        [b]
    );
}

#[test]
fn a_managed_key_account_s_quarantine_follows_the_live_key_the_same_way() {
    // The managed-key axis: an empty live key may be the bound one, another key is not.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let k = fx.add_api_key(API_KEY);
    fx.switch_to(&k, false).unwrap(); // k is live, on its managed key
    fx.quarantine(&k, "invalid_grant", &vault_fp(&fx, &k));
    fx.put_vault(&k, OTHER_API_KEY.as_bytes());
    let release = || {
        fx.engine
            .release_unbound_quarantines(&fx.provider(), "cli")
            .unwrap()
    };
    fx.put_managed_key(b"");
    assert!(release().is_empty(), "an empty key is not another key");
    fx.put_managed_key(API_KEY.as_bytes());
    assert!(release().is_empty(), "still the bound key");
    fx.put_managed_key(OTHER_API_KEY.as_bytes());
    assert_eq!(release(), [k]);
}

/// §14.1: an interruption while collecting ends the command with the signal. It is not a
/// usage failure, so the strategy never goes on to rank, and nothing is switched.
#[cfg(feature = "test-hooks")]
#[test]
fn an_interrupted_collection_ends_the_strategy_with_the_signal_and_no_switch() {
    for strategy in [UsageStrategy::Best, UsageStrategy::NextAvailable] {
        let fx = Fx::new();
        let (a, b, c) = three(&fx);
        for id in [&a, &b, &c] {
            record_at(&fx, id, &reading(10.0, 10.0, 0.0), T0 - 200, T0 - 200);
        }
        let cancel = fx.engine.cancel().clone();
        fx.engine.on_point(
            "usage-before-send",
            Box::new(move || cancel.request(libc::SIGINT)),
        );
        let err = fx
            .engine
            .switch(request(&fx, usage(strategy, None)))
            .unwrap_err();
        assert_eq!(err.signal(), Some(libc::SIGINT), "{strategy:?}: {err}");
        assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
        assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-c"));
        assert!(
            fx.engine
                .store()
                .unwrap()
                .events()
                .unwrap()
                .iter()
                .all(|e| e.kind != "switch")
        );
    }
}
