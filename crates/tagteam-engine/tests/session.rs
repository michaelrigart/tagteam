//! §12.5 and §12.6: session ownership from launch reservations and session records, the gate's
//! step 2 on it, and §10.3's destructive guard and profile removal.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::{Fx, LSTART, due, token_requests};
use tagteam_core::AccountId;
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::session::SessionState;
use tagteam_provider::Provider;
use tagteam_provider::liveness::{FakeProcess, parse_lstart};
use tagteam_provider::profile::LAUNCH_DIR;

fn state(fx: &Fx, id: &AccountId) -> SessionState {
    let row = fx.engine.store().unwrap().account(id).unwrap().unwrap();
    fx.engine.session_state(fx.cc.as_ref(), &row).unwrap()
}

/// The gate on `id`, with the vault's current bytes as the caller's snapshot.
fn gate(fx: &Fx, id: &AccountId) -> GateOutcome {
    let snapshot = fx.vault_bytes(id).unwrap();
    fx.engine
        .refresh_stored(fx.cc.as_ref(), id, &snapshot)
        .unwrap()
}

#[test]
fn an_account_without_a_profile_has_no_session() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let s = state(&fx, &a);
    assert_eq!(s, SessionState::NoProfile);
    assert!(!s.owned());
    assert_eq!(s.profile(), None);
}

#[test]
fn a_profile_with_nothing_running_is_quiescent() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&a);
    let s = state(&fx, &a);
    assert_eq!(
        s,
        SessionState::Quiescent {
            profile: dir.clone()
        }
    );
    assert!(!s.owned());
    assert_eq!(s.profile(), Some(dir.as_path()));
}

#[test]
fn a_held_reservation_owns_the_account_and_a_released_one_does_not() {
    // §12.5: a reservation is live while its file is locked, and only then.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&a);
    let held = fx.hold_reservation(&dir);
    let s = state(&fx, &a);
    assert_eq!(
        s,
        SessionState::Owned {
            profile: dir.clone()
        }
    );
    assert!(s.owned());
    drop(held);
    assert!(
        fs::read_dir(dir.join(LAUNCH_DIR)).unwrap().next().is_some(),
        "the file is still there"
    );
    assert_eq!(state(&fx, &a), SessionState::Quiescent { profile: dir });
}

#[test]
fn a_live_record_of_any_kind_owns_the_account() {
    // §12.6: `bg` and `daemon` records count, so CC's daemon keeps the account session-owned
    // after the last `run` (§15.2 "Liveness").
    for kind in ["interactive", "bg", "daemon"] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let dir = fx.make_profile(&a);
        fx.live_record(&dir, 4242, kind);
        assert_eq!(
            state(&fx, &a),
            SessionState::Owned { profile: dir },
            "{kind}"
        );
    }
}

#[test]
fn a_dead_or_recycled_record_does_not_own_the_account() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&a);
    fx.dead_record(&dir, 4242);
    assert_eq!(
        state(&fx, &a),
        SessionState::Quiescent {
            profile: dir.clone()
        }
    );
    // Review Focus 2's case: the pid runs again, started at another time, and it is not a
    // claude process (Decision 16).
    fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART).map(|s| s + 3_600),
            mentions_launch: Some(false),
            ..FakeProcess::default()
        },
    );
    assert_eq!(
        state(&fx, &a),
        SessionState::Quiescent {
            profile: dir.clone()
        }
    );
    // The same mismatch on a process that mentions `claude` may be the session itself, under a
    // start time the wall clock moved (Decision 16): it still owns the account.
    fx.process.set(
        4242,
        FakeProcess {
            exists: Some(true),
            start_time_s: parse_lstart(LSTART).map(|s| s + 3_600),
            mentions_launch: Some(true),
            ..FakeProcess::default()
        },
    );
    assert_eq!(state(&fx, &a), SessionState::Owned { profile: dir });
}

#[test]
fn an_unreadable_record_counts_as_owned() {
    // §12.6: a malformed record is unreadable, and unreadable counts as owned (§10.3).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&a);
    fx.dead_record(&dir, 4242);
    fx.plant_record(&dir, "torn", b"{\"pid\":");
    let s = state(&fx, &a);
    let SessionState::Unreadable { profile, detail } = &s else {
        panic!("{s:?}")
    };
    assert_eq!(profile, &dir);
    assert!(detail.contains("torn.json"), "{detail}");
    assert!(s.owned());
}

#[test]
fn a_directory_that_cannot_be_listed_counts_as_owned() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&a);
    for blocked in [dir.join(LAUNCH_DIR), fx.cc.session_records_dir(&dir)] {
        fs::create_dir_all(&blocked).unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let s = state(&fx, &a);
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(s, SessionState::Unreadable { .. }),
            "{}: {s:?}",
            blocked.display()
        );
        assert!(s.owned());
    }
}

#[test]
fn the_gate_leaves_a_session_owned_token_alone() {
    // §7.3 step 2: a held reservation, a live record of any kind, or an unreadable record.
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let dir = fx.make_profile(&a);
    let held = fx.hold_reservation(&dir);
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Owned(OwnedBy::Session)
    ));
    drop(held);
    let record = fx.live_record(&dir, 4242, "daemon");
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Owned(OwnedBy::Session)
    ));
    fs::remove_file(record).unwrap();
    fx.plant_record(&dir, "torn", b"{\"pid\":");
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Owned(OwnedBy::Session)
    ));
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn a_session_that_ended_leaves_the_token_to_the_gate_again() {
    let fx = Fx::new();
    let a = due(&fx);
    fx.script_refresh(Some("rt-a2"));
    let dir = fx.make_profile(&a);
    drop(fx.hold_reservation(&dir));
    fx.dead_record(&dir, 4242);
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(token_requests(&fx), 1);
}
