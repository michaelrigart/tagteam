#![cfg(feature = "test-hooks")]

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use common::{Fx, mutation_lock_free};
use serde_json::json;
use tagteam_cc::{ItemKind, keychain_service};
use tagteam_core::AccountId;
use tagteam_engine::oracle::Oracle;
use tagteam_engine::store::JournalRow;
use tagteam_engine::switch::{SwitchOutcome, SwitchReason, SwitchRequest, SwitchTarget};
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::{Credential, Env, Identity, ProcessStamp, Provider};

fn request(fx: &Fx, target: SwitchTarget, force: bool) -> SwitchRequest {
    SwitchRequest {
        provider: fx.provider(),
        target,
        force,
        source: "cli",
    }
}

fn switch_to(fx: &Fx, id: &AccountId, force: bool) -> Result<SwitchOutcome, EngineError> {
    fx.engine
        .switch(request(fx, SwitchTarget::Account(id.clone()), force))
}

/// Runs a switch that is expected to panic at an injected point.
fn switch_panicking(fx: &Fx, id: &AccountId, force: bool) {
    let r = catch_unwind(AssertUnwindSafe(|| switch_to(fx, id, force)));
    assert!(r.is_err(), "the injected panic must reach the caller");
}

/// Three accounts, c live and active, so a bare rotation plans position 1 (a). At the
/// `planned` point, before any lock, another tagteam process moves a to position 2, then
/// runs `then`: under the locks, position 1 holds b, and the rotation must be planned again.
fn three_with_a_move_while_planned(
    fx: &Fx,
    engine: &Engine,
    then: impl Fn() + Send + Sync + 'static,
) -> (AccountId, AccountId, AccountId) {
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c");
    let other = fx.engine_with_env(fx.env.clone());
    let moved = a.clone();
    engine.on_point(
        "planned",
        Box::new(move || {
            drop(other.move_to(&moved, 2));
            then();
        }),
    );
    (a, b, c)
}

/// An undecidable row left by a dead process (§9.6), which only `switch --force` may settle.
/// Its fingerprints match nothing, so recovery can never decide it either way.
fn undecidable_row(fx: &Fx, to: &AccountId) -> JournalRow {
    let row = JournalRow {
        provider: fx.provider(),
        holder: ProcessStamp {
            pid: 999_999,
            start: 0,
        },
        from_id: None,
        to_id: to.clone(),
        from_fp: Some("sha256:matches-nothing".into()),
        from_identity: Some(json!({"emailAddress": "gone@x.co"})),
        to_fp: "sha256:matches-nothing-either".into(),
        started_at: 1,
        prior: None,
    };
    fx.engine.store().unwrap().insert_journal(&row).unwrap();
    row
}

#[test]
fn an_error_at_any_step_restores_every_byte() {
    for point in ["after-journal", "after-credential", "after-identity"] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        fx.add("b@x.co", "rt-b");
        let cfg_before = std::fs::read(fx.paths().global_config).unwrap();
        let cred_before = fx.live_credential();
        fx.engine.fail_at(Some(point));
        let err = switch_to(&fx, &a, false).unwrap_err();
        assert!(matches!(err, EngineError::RolledBack(_)), "{point}: {err}");
        assert_eq!(
            std::fs::read(fx.paths().global_config).unwrap(),
            cfg_before,
            "{point}"
        );
        assert_eq!(fx.live_credential(), cred_before, "{point}");
        let store = fx.engine.store().unwrap();
        assert!(store.journal(&fx.provider()).unwrap().is_none(), "{point}");
        assert_ne!(store.active(&fx.provider()).unwrap(), Some(a), "{point}");
    }
}

#[test]
fn a_rotation_is_replanned_when_positions_move_during_the_wait() {
    let fx = Fx::new();
    let (_, b, _) = three_with_a_move_while_planned(&fx, &fx.engine, || {});
    let out = fx
        .engine
        .switch(request(&fx, SwitchTarget::Rotation, false))
        .unwrap();
    assert_eq!(
        out.to.unwrap().id,
        b,
        "position 1 holds b by the time the locks are held"
    );
}

#[test]
fn a_rotation_down_to_one_switchable_account_becomes_a_noop() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let c = fx.add("c@x.co", "rt-c");
    let other = fx.engine_with_env(fx.env.clone());
    fx.engine.on_point(
        "planned",
        Box::new(move || {
            drop(other.set_disabled(&b, true));
            drop(other.set_disabled(&c, true));
        }),
    );
    let out = fx
        .engine
        .switch(request(&fx, SwitchTarget::Rotation, false))
        .unwrap();
    assert_eq!(out.reason, SwitchReason::OnlyOneAccount);
}

#[test]
fn a_login_that_becomes_unmanaged_during_the_wait_is_planned_again_as_a_noop() {
    // §9.2: the switch re-plans into the unmanaged-account no-op rather than failing.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let path = fx.paths().global_config;
    fx.engine.on_point(
        "planned",
        Box::new(move || {
            common::splice_oauth_account(&path, &Fx::oauth_account("stranger@x.co"));
        }),
    );
    let out = switch_to(&fx, &a, false).unwrap();
    assert_eq!(
        (out.switched, out.reason, out.unmanaged_email.as_deref()),
        (false, SwitchReason::UnmanagedAccount, Some("stranger@x.co"))
    );
    assert_eq!(fx.live_email().as_deref(), Some("stranger@x.co"));
}

#[test]
fn a_rotation_target_removed_during_the_wait_is_planned_again() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live and active: the rotation plans a
    let other = fx.engine_with_env(fx.env.clone());
    fx.engine
        .on_point("planned", Box::new(move || drop(other.remove(&a))));
    let out = fx
        .engine
        .switch(request(&fx, SwitchTarget::Rotation, false))
        .unwrap();
    assert_eq!(out.to.unwrap().id, b);
}

#[test]
fn a_direct_target_removed_during_the_wait_is_reported_after_planning_again() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let other = fx.engine_with_env(fx.env.clone());
    let removed = a.clone();
    fx.engine
        .on_point("planned", Box::new(move || drop(other.remove(&removed))));
    assert!(matches!(
        switch_to(&fx, &a, false),
        Err(EngineError::NoSuchAccount(id)) if id == a.as_str()
    ));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

/// Records, for every oracle call, whether tagteam's mutation lock was free at the time.
struct GuardProbe {
    env: Env,
    calls: Mutex<Vec<bool>>,
}

impl Oracle for GuardProbe {
    fn resolve(&self, _p: &dyn Provider, _c: &Credential) -> Option<Identity> {
        self.calls
            .lock()
            .unwrap()
            .push(mutation_lock_free(&self.env));
        None
    }
}

#[test]
fn a_replan_never_asks_the_oracle_under_the_mutation_lock() {
    // §4.3, §9.4: no network once the locks are taken, so a retry re-plans without the oracle.
    let fx = Fx::new();
    let probe = Arc::new(GuardProbe {
        env: fx.env.clone(),
        calls: Mutex::new(vec![]),
    });
    let engine = fx.engine_with_oracle(probe.clone());
    let (_, b, c) = three_with_a_move_while_planned(&fx, &engine, || {});
    fx.rotate_live("rt-c2"); // diverged from the vault: the plan asks the oracle about it
    let out = engine
        .switch(request(&fx, SwitchTarget::Rotation, false))
        .unwrap();
    assert_eq!(out.to.unwrap().id, b, "the rotation was planned again");
    assert_eq!(
        *probe.calls.lock().unwrap(),
        [true],
        "asked once, before the mutation lock"
    );
    assert_eq!(fx.vault_refresh_token(&c).as_deref(), Some("rt-c2"));
}

#[test]
fn a_replan_keeps_the_mutation_lock() {
    // §9.4 step 1: a retry releases every lock except the mutation lock, so no other tagteam
    // writer can run between two attempts. The re-plan reads the vault, and every read from
    // the `planned` point on must find the lock held.
    let fx = Fx::new();
    let armed = Arc::new(AtomicBool::new(false));
    let free = Arc::new(Mutex::new(vec![]));
    let (env, is_armed, record) = (fx.env.clone(), armed.clone(), free.clone());
    let engine = fx.engine_with_vault_probe(move |_| {
        if is_armed.load(Ordering::SeqCst) {
            record.lock().unwrap().push(mutation_lock_free(&env));
        }
    });
    let (_, b, _) =
        three_with_a_move_while_planned(&fx, &engine, move || armed.store(true, Ordering::SeqCst));
    let out = engine
        .switch(request(&fx, SwitchTarget::Rotation, false))
        .unwrap();
    assert_eq!(out.to.unwrap().id, b, "the rotation was planned again");
    let free = free.lock().unwrap();
    assert!(
        !free.is_empty() && free.iter().all(|f| !f),
        "every vault read after planning ran under the mutation lock: {free:?}"
    );
}

#[test]
fn a_replan_keeps_the_pre_lock_oracle_answer_for_unchanged_bytes() {
    // §9.4 step 4: the pre-lock answer holds as long as the live bytes are the ones it was
    // asked about, re-plan or not. Dropping it would capture this stranger's token into c's
    // vault as an unresolved generation.
    let fx = Fx::new();
    let (_, b, c) = three_with_a_move_while_planned(&fx, &fx.engine, || {});
    fx.rotate_live("someone-elses-token");
    let stranger = fx
        .cc
        .parse_identity(&json!({"emailAddress": "z@x.co", "accountUuid": "uuid-z"}))
        .unwrap();
    fx.oracle.set(Some(stranger));
    let out = fx
        .engine
        .switch(request(&fx, SwitchTarget::Rotation, false))
        .unwrap();
    assert_eq!(out.to.unwrap().id, b);
    assert_eq!(fx.vault_refresh_token(&c).as_deref(), Some("rt-c"));
    assert!(out.warnings.iter().any(|w| w.contains("displaced")));
}

#[test]
fn add_refuses_when_the_identity_changes_while_it_waits() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let path = fx.paths().global_config;
    fx.engine.on_point(
        "add-verified",
        Box::new(move || {
            let changed = json!({"emailAddress": "me@work.co", "organizationUuid": "", "accountUuid": "uuid-recycled"});
            common::splice_oauth_account(&path, &changed);
        }),
    );
    assert!(matches!(
        fx.engine.add_live(fx.add_options()),
        Err(EngineError::LiveMoved)
    ));
}

#[test]
fn a_panic_after_a_live_write_rolls_back_through_drop() {
    for point in ["panic:after-credential", "panic:after-identity"] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        fx.add("b@x.co", "rt-b");
        let cfg_before = std::fs::read(fx.paths().global_config).unwrap();
        let cred_before = fx.live_credential();
        fx.engine.fail_at(Some(point));
        switch_panicking(&fx, &a, false);
        assert_eq!(
            std::fs::read(fx.paths().global_config).unwrap(),
            cfg_before,
            "{point}"
        );
        assert_eq!(fx.live_credential(), cred_before, "{point}");
        // §9.4 step 10: every undo succeeded, so the row goes, exactly as on an error.
        assert!(
            fx.engine
                .store()
                .unwrap()
                .journal(&fx.provider())
                .unwrap()
                .is_none(),
            "{point}"
        );
    }
}

#[test]
fn a_rolled_back_forced_switch_puts_the_superseded_row_back() {
    // §9.6 forced-switch put-back, on an error and through `Drop`.
    for point in ["after-identity", "panic:after-identity"] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        let prior = undecidable_row(&fx, &b);
        fx.engine.fail_at(Some(point));
        if point.starts_with("panic:") {
            switch_panicking(&fx, &a, true);
        } else {
            let err = switch_to(&fx, &a, true).unwrap_err();
            assert!(matches!(err, EngineError::RolledBack(_)), "{point}: {err}");
        }
        let store = fx.engine.store().unwrap();
        assert_eq!(
            store.journal(&fx.provider()).unwrap(),
            Some(prior),
            "{point}"
        );
        // A forced switch that lands settles the undecidable row for good.
        fx.engine.fail_at(None);
        switch_to(&fx, &a, true).unwrap();
        assert!(store.journal(&fx.provider()).unwrap().is_none(), "{point}");
    }
}

#[test]
fn a_failed_rollback_keeps_the_journal_for_recovery() {
    // The credential undo cannot write the outgoing credential back, on an error and through
    // `Drop` alike: only §9.6 recovery may settle what is left.
    for point in ["after-identity", "panic:after-identity"] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        fx.add("b@x.co", "rt-b");
        let kc = fx.kc.clone();
        let svc = keychain_service(&fx.env, ItemKind::OAuth);
        fx.engine.on_point(
            "after-identity",
            Box::new(move || kc.set_fail_write(&svc, true)),
        );
        fx.engine.fail_at(Some(point));
        if point.starts_with("panic:") {
            switch_panicking(&fx, &a, false);
        } else {
            let err = switch_to(&fx, &a, false).unwrap_err();
            assert!(
                matches!(err, EngineError::RollbackFailed { .. }),
                "{point}: {err}"
            );
        }
        let store = fx.engine.store().unwrap();
        assert!(store.journal(&fx.provider()).unwrap().is_some(), "{point}");
        assert_ne!(store.active(&fx.provider()).unwrap(), Some(a), "{point}");
    }
}

#[test]
fn a_panic_inside_a_live_write_keeps_the_journal_for_recovery() {
    // The provider puts its own partial write back while unwinding, but the engine cannot see
    // whether that worked, so the row stays for §9.6 recovery to confirm.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let cfg_before = std::fs::read(fx.paths().global_config).unwrap();
    let cred_before = fx.live_credential();
    // Writing OAuth clears the managed-key axis after the credential entry is written.
    fx.kc
        .set_panic_on_delete(&keychain_service(&fx.env, ItemKind::ManagedKey), true);
    switch_panicking(&fx, &a, false);
    assert_eq!(std::fs::read(fx.paths().global_config).unwrap(), cfg_before);
    assert_eq!(fx.live_credential(), cred_before);
    let store = fx.engine.store().unwrap();
    assert!(store.journal(&fx.provider()).unwrap().is_some());
}
