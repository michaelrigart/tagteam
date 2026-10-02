//! §12.5 and §12.6: session ownership from launch reservations and session records, the gate's
//! step 2 on it, and §10.3's destructive guard and profile removal.

mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::sync::Mutex;

use common::{API_KEY, Fx, LSTART, capture_logs, credential, due, journal, token_requests};
use tagteam_cc::live::Platform;
use tagteam_core::AccountId;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::session::SessionState;
use tagteam_engine::switch::SwitchReason;
use tagteam_provider::FlockGuard;
use tagteam_provider::liveness::{FakeProcess, parse_lstart};
use tagteam_provider::profile::{LAUNCH_DIR, MARKER_FILE, ProfileMarker, canonical_profile_path};
use tagteam_provider::{Provider, Read};

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

#[test]
fn remove_refuses_while_a_session_owns_the_account() {
    // §10.3 Guard.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let held = fx.hold_reservation(&dir);
    let err = fx.engine.remove(&a).unwrap_err();
    assert_eq!(err.kind(), "session-owned", "{err}");
    assert!(err.to_string().contains("tagteam run"), "{err}");
    assert!(fx.vault_bytes(&a).is_some());
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_some());
    assert!(dir.join(MARKER_FILE).exists());
    drop(held);
    fx.engine.remove(&a).unwrap();
}

#[test]
fn remove_refuses_while_a_session_record_is_unreadable() {
    // §12.6: unreadable records block destructive operations.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    fx.plant_record(&dir, "torn", b"[1,");
    assert_eq!(fx.engine.remove(&a).unwrap_err().kind(), "session-owned");
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_some());
}

#[test]
fn add_over_a_session_owned_occupant_refuses_before_writing_anything() {
    // §10.3 Guard: the occupant is the account `add --position` removes.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let _held = fx.hold_reservation(&fx.make_profile(&a));
    fx.login("c@x.co", "rt-c");
    let err = fx
        .engine
        .add_live(AddOptions {
            position: Some(1),
            yes: true,
            ..fx.add_options()
        })
        .unwrap_err();
    assert_eq!(err.kind(), "session-owned", "{err}");
    let err = fx
        .engine
        .add_token(AddTokenOptions {
            position: Some(1),
            yes: true,
            ..fx.add_token_options(API_KEY)
        })
        .unwrap_err();
    assert_eq!(err.kind(), "session-owned", "{err}");
    let rows = fx.engine.store().unwrap().accounts(&fx.provider()).unwrap();
    let held: Vec<(u32, &str)> = rows
        .iter()
        .map(|r| (r.position, r.label.as_str()))
        .collect();
    assert_eq!(held, [(1, "a@x.co"), (2, "b@x.co")]);
    assert!(fx.vault_bytes(&a).is_some());
}

#[test]
fn replacing_a_session_owned_accounts_login_is_not_destructive() {
    // §10.3 lists what is destructive; an explicit replacement only stale-marks a running
    // profile, which is never touched (§12.5).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let _held = fx.hold_reservation(&dir);
    fx.login("a@x.co", "rt-a2");
    fx.engine.add_live(fx.add_options()).unwrap();
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert!(
        dir.join(MARKER_FILE).exists(),
        "the running profile is untouched"
    );
}

#[test]
fn move_is_not_destructive() {
    // §10.3: positions are display order only, and a profile is keyed by the account's ID.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let _held = fx.hold_reservation(&fx.make_profile(&a));
    assert_eq!(fx.engine.move_to(&a, 2).unwrap().position, 2);
}

#[test]
fn remove_deletes_the_profile_its_item_first_and_its_links_as_links() {
    // §10.3: within the profile, its hashed item goes before its directory.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let projects = fx.env.home.join(".claude/projects");
    symlink(&projects, dir.join("projects")).unwrap();
    let (svc, acct) = fx.profile_item(&dir);
    fx.kc.put(&svc, &acct, &credential("a@x.co", "rt-a2"));
    fx.engine.remove(&a).unwrap();
    assert!(fs::symlink_metadata(&dir).is_err(), "the profile is gone");
    assert_eq!(fx.kc.get(&svc, &acct), None, "its hashed item is gone");
    assert!(
        projects.join("-work-app/memory/MEMORY.md").exists(),
        "nothing a link points at is touched"
    );
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
}

#[test]
fn remove_deletes_the_item_under_the_spelling_the_marker_records() {
    // §12.2 "One spelling": never a spelling derived again. Here the data directory moved
    // since the profile was exported, so the canonical spelling is another one.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let Read::Present(mut marker) = ProfileMarker::read(&dir) else {
        panic!("the fixture wrote a marker")
    };
    let canonical = marker.config_dir.clone();
    marker.config_dir = "/old/data/tagteam/sessions/a".into();
    marker.write(&dir).unwrap();
    let (old_svc, acct) = fx.item_for_spelling(&marker.config_dir);
    let (canonical_svc, _) = fx.item_for_spelling(&canonical);
    fx.kc.put(&old_svc, &acct, b"recorded");
    fx.kc.put(&canonical_svc, &acct, b"derived");
    fx.engine.remove(&a).unwrap();
    assert_eq!(fx.kc.get(&old_svc, &acct), None);
    assert_eq!(
        fx.kc.get(&canonical_svc, &acct).as_deref(),
        Some(&b"derived"[..])
    );
}

#[test]
fn remove_with_an_unreadable_marker_deletes_the_current_item_and_warns() {
    // Decision 12: refusing would leave an account that cannot be removed.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let canonical = fx
        .cc
        .profile_spelling(&canonical_profile_path(&dir).unwrap());
    let (svc, acct) = fx.item_for_spelling(&canonical);
    fx.kc.put(&svc, &acct, b"x");
    fs::write(dir.join(MARKER_FILE), "{ torn").unwrap();
    let (result, logs) = capture_logs(|| fx.engine.remove(&a));
    result.unwrap();
    assert_eq!(fx.kc.get(&svc, &acct), None);
    assert!(fs::symlink_metadata(&dir).is_err());
    assert!(
        logs.iter()
            .any(|l| l.contains("WARN") && l.contains("older spelling")),
        "{logs:?}"
    );
}

#[test]
fn remove_never_trusts_a_marker_that_names_another_account() {
    // A marker copied from a's profile into b's names a's spelling. Removing b must delete b's
    // item under b's own canonical spelling, and leave a's item, which a running session of a
    // may be using, alone.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: c, so a and b are both removable
    let dir_a = fx.make_profile(&a);
    let dir_b = fx.make_profile(&b);
    let Read::Present(marker_a) = ProfileMarker::read(&dir_a) else {
        panic!("the fixture wrote a marker")
    };
    fs::copy(dir_a.join(MARKER_FILE), dir_b.join(MARKER_FILE)).unwrap();
    let (a_svc, acct) = fx.item_for_spelling(&marker_a.config_dir);
    let b_spelling = fx
        .cc
        .profile_spelling(&canonical_profile_path(&dir_b).unwrap());
    let (b_svc, _) = fx.item_for_spelling(&b_spelling);
    fx.kc.put(&a_svc, &acct, b"a's");
    fx.kc.put(&b_svc, &acct, b"b's");

    let (result, logs) = capture_logs(|| fx.engine.remove(&b));
    result.unwrap();

    assert_eq!(fx.kc.get(&a_svc, &acct).as_deref(), Some(&b"a's"[..]));
    assert_eq!(fx.kc.get(&b_svc, &acct), None);
    assert!(fs::symlink_metadata(&dir_b).is_err());
    assert!(fs::symlink_metadata(&dir_a).is_ok());
    assert!(
        logs.iter()
            .any(|l| l.contains("WARN") && l.contains("names another account")),
        "{logs:?}"
    );
}

#[test]
fn remove_of_a_dangling_profile_link_skips_the_item_and_removes_the_link() {
    // The profile path resolves to nothing and has no marker, so there is no spelling to name
    // an item from. Once the vault is gone, a stray path must not leave `remove` unable to
    // finish: the link goes as a link, the row goes, and no Keychain item is touched.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let rescue = fx.plant_rescue(
        &a,
        &common::vault_fp(&fx, &a),
        &credential("a@x.co", "rt-a2"),
    );
    let dir = fx.profile_dir(&a);
    fs::create_dir_all(dir.parent().unwrap()).unwrap();
    let target = fx.dir.path().join("moved-away/profile");
    symlink(&target, &dir).unwrap();
    let (svc, acct) = fx.item_for_spelling("/some/other/spelling");
    fx.kc.put(&svc, &acct, b"someone else's");
    let before = fx.kc.items();
    assert_eq!(
        state(&fx, &a),
        SessionState::Quiescent {
            profile: dir.clone()
        }
    );

    let (result, logs) = capture_logs(|| fx.engine.remove(&a));
    result.unwrap();

    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
    assert!(fx.vault_bytes(&a).is_none());
    assert!(!rescue.exists());
    assert!(fs::symlink_metadata(&dir).is_err(), "the link is gone");
    assert!(
        fs::symlink_metadata(target.parent().unwrap()).is_err(),
        "nothing was created where the link pointed"
    );
    // The vault's own entries went; every other item, the agent's included, is as it was.
    let agent_items = |items: std::collections::BTreeMap<(String, String), Vec<u8>>| {
        items
            .into_iter()
            .filter(|((service, _), _)| service != "tagteam")
            .collect::<Vec<_>>()
    };
    assert_eq!(agent_items(fx.kc.items()), agent_items(before));
    assert_eq!(
        fx.kc.get(&svc, &acct).as_deref(),
        Some(&b"someone else's"[..])
    );
    assert!(
        logs.iter()
            .any(|l| l.contains("WARN") && l.contains("older spelling")),
        "{logs:?}"
    );
}

#[test]
fn a_remove_that_stops_at_the_profile_has_already_deleted_the_vault_and_keeps_the_row() {
    // §10.3's order: the vault (and any rescue) goes before the profile, so a stop at the
    // profile leaves no older generation behind a newer profile one; the row goes last, so the
    // account stays listed and running `remove` again finishes.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    let (svc, acct) = fx.profile_item(&dir);
    fx.kc.put(&svc, &acct, &credential("a@x.co", "rt-a2"));
    fx.kc.set_fail_delete(&svc, true);
    assert!(fx.engine.remove(&a).is_err());
    assert!(fx.vault_bytes(&a).is_none(), "the vault went first");
    assert!(
        dir.join(MARKER_FILE).exists(),
        "the directory goes only after the item"
    );
    assert!(
        fx.engine.store().unwrap().account(&a).unwrap().is_some(),
        "the row goes last"
    );
    fx.kc.set_fail_delete(&svc, false);
    fx.engine.remove(&a).unwrap();
    assert_eq!(fx.kc.get(&svc, &acct), None);
    assert!(fs::symlink_metadata(&dir).is_err());
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
}

#[test]
fn on_linux_remove_deletes_the_profile_directory_and_touches_no_keychain() {
    let fx = Fx::with_platform(Platform::Linux);
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    fx.set_profile_credential(&dir, &credential("a@x.co", "rt-a2"));
    fx.engine.remove(&a).unwrap();
    assert!(fs::symlink_metadata(&dir).is_err());
    assert!(fx.kc.items().is_empty(), "Linux has no Keychain");
}

#[cfg(feature = "test-hooks")]
#[test]
fn a_remove_with_a_profile_leaves_the_next_switch_able_to_roll_back() {
    // Task 7's ledger: deleting a profile's item must leave no stale ledger entry behind, or
    // the next operation in this process could not roll back.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: c
    let dir = fx.make_profile(&a);
    let (svc, acct) = fx.profile_item(&dir);
    fx.kc.put(&svc, &acct, &credential("a@x.co", "rt-a2"));
    fx.engine.remove(&a).unwrap();
    assert_eq!(fx.kc.get(&svc, &acct), None);

    fx.engine.fail_at(Some("after-credential"));
    let err = fx.switch_to(&b, false).unwrap_err();
    assert!(
        matches!(err, tagteam_engine::EngineError::RolledBack(_)),
        "{err}"
    );
    assert_eq!(fx.live_email().as_deref(), Some("c@x.co"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-c"));
    assert!(
        fx.engine
            .store()
            .unwrap()
            .journal(&fx.provider())
            .unwrap()
            .is_none()
    );
}

// §9.2–§9.4: switch's session rules.

/// A vault probe that starts a session for `id` (a held reservation in `profile`) the first
/// time `id`'s vault is read: after planning has checked the account, and before freshening
/// refreshes it.
fn start_session_on_first_read(
    id: &AccountId,
    profile: &Path,
) -> impl Fn(&str) + Send + Sync + 'static {
    let key = id.as_str().to_owned();
    let launch = profile.join(LAUNCH_DIR).join("4242.lock");
    let held = Mutex::new(None);
    move |read: &str| {
        let mut held = held.lock().unwrap();
        if read == key && held.is_none() {
            *held = FlockGuard::try_lock(&launch).unwrap();
        }
    }
}

#[test]
fn a_session_owned_target_is_refused_with_or_without_force() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let _held = fx.hold_reservation(&fx.make_profile(&a));
    for force in [false, true] {
        let err = fx.switch_to(&a, force).unwrap_err();
        assert_eq!(err.kind(), "session-owned", "force {force}: {err}");
        assert!(err.to_string().contains("tagteam run"), "{err}");
    }
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    assert!(journal(&fx).is_none());
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_target_with_an_unreadable_session_record_is_refused() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&a);
    fx.plant_record(&dir, "torn", b"{\"pid\":");
    assert_eq!(fx.switch_to(&a, false).unwrap_err().kind(), "session-owned");
}

#[test]
fn a_rotation_skips_a_session_owned_candidate() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live: the walk starts after it, at a
    let _held = fx.hold_reservation(&fx.make_profile(&a));
    let out = fx.engine.switch(fx.rotation_request(false)).unwrap();
    assert_eq!(out.to.map(|t| t.id), Some(b));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn a_rotation_whose_only_alternative_is_session_owned_stays_put() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let _held = fx.hold_reservation(&fx.make_profile(&a));
    let out = fx.engine.switch(fx.rotation_request(false)).unwrap();
    assert_eq!(out.reason, SwitchReason::OnlyOneAccount);
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn a_session_that_starts_before_the_gate_refuses_a_direct_switch_and_sends_nothing() {
    // §7.2's table: `Owned` by a session is §9.2's refusal.
    let fx = Fx::new();
    let a = due(&fx); // a inactive and due, b live
    fx.script_refresh(Some("rt-a2"));
    let dir = fx.make_profile(&a);
    let engine = fx.engine_with_vault_probe(start_session_on_first_read(&a, &dir));
    let err = engine.switch(fx.switch_request(&a, false)).unwrap_err();
    assert_eq!(err.kind(), "session-owned", "{err}");
    assert_eq!(
        token_requests(&fx),
        0,
        "the gate left the session's token alone"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
}

#[test]
fn a_session_that_starts_before_the_gate_makes_a_rotation_move_on() {
    // §9.3: the rotation plans again, and its walk passes over the account a session took.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c"); // live
    fx.expire_access(&a);
    let dir = fx.make_profile(&a);
    let engine = fx.engine_with_vault_probe(start_session_on_first_read(&a, &dir));
    let out = engine.switch(fx.rotation_request(false)).unwrap();
    assert_eq!(out.to.map(|t| t.id), Some(b));
    assert_eq!(token_requests(&fx), 0);
}

#[cfg(feature = "test-hooks")]
mod under_the_locks {
    use std::sync::Arc;

    use super::*;

    /// Starts a session for the account whose profile is `profile` once the switch has planned,
    /// before it takes the mutation lock. The guard is kept in `slot`.
    fn on_planned(fx: &Fx, profile: &Path) -> Arc<Mutex<Option<FlockGuard>>> {
        let slot: Arc<Mutex<Option<FlockGuard>>> = Arc::default();
        let (held, launch) = (slot.clone(), profile.join(LAUNCH_DIR).join("4242.lock"));
        fx.engine.on_point(
            "planned",
            Box::new(move || {
                let mut held = held.lock().unwrap();
                if held.is_none() {
                    *held = FlockGuard::try_lock(&launch).unwrap();
                }
            }),
        );
        slot
    }

    #[test]
    fn a_session_that_starts_after_planning_refuses_a_direct_switch() {
        // §9.4 step 1.
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        fx.add("b@x.co", "rt-b");
        let held = on_planned(&fx, &fx.make_profile(&a));
        let err = fx.switch_to(&a, false).unwrap_err();
        assert_eq!(err.kind(), "session-owned", "{err}");
        assert!(
            held.lock().unwrap().is_some(),
            "the session started after planning"
        );
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
        assert!(journal(&fx).is_none());
    }

    #[test]
    fn a_session_that_starts_after_planning_makes_a_rotation_plan_again() {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        fx.add("c@x.co", "rt-c"); // live: the plan picks a
        let _held = on_planned(&fx, &fx.make_profile(&a));
        let out = fx.engine.switch(fx.rotation_request(false)).unwrap();
        assert_eq!(out.to.map(|t| t.id), Some(b));
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    }
}
