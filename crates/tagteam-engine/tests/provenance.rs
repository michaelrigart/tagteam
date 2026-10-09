//! §12.5 "Profile provenance" and "Lazy capture" through the refresh gate (§7.3 step 3), every
//! row of the table with its request count (§15.2 "Provenance").

mod common;

use std::fs;
use std::path::Path;

use common::{
    Fx, cred_at, credential, due, fp, journal, prev_refresh_token, quiescent, sent_refresh_tokens,
    token_requests, two_accounts,
};
use serde_json::{Value, json};
use tagteam_cc::live::Platform;
use tagteam_core::AccountId;
use tagteam_engine::refresh::GateOutcome;
use tagteam_provider::profile::{
    MARKER_FILE, ProfileMarker, SEED_FILE, Seed, canonical_profile_path,
};
use tagteam_provider::{Clock, Provider, Read};

fn expires_at(bytes: &[u8]) -> i64 {
    let v: Value = serde_json::from_slice(bytes).unwrap();
    v["claudeAiOauth"]["expiresAt"].as_i64().unwrap()
}

/// An explicit replacement that landed since `id`'s profile was bootstrapped: the account's
/// `login_epoch` moved on, which stale-marks the profile (§12.5).
fn bump_epoch(fx: &Fx, id: &AccountId) {
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "UPDATE accounts SET login_epoch = login_epoch + 1 WHERE id = ?1",
            [id.as_str()],
        )
        .unwrap();
}

fn seed_of(dir: &Path) -> Seed {
    match Seed::read(dir) {
        Read::Present(seed) => seed,
        other => panic!("{other:?}"),
    }
}

/// The gate on `id`, with the vault's current bytes as the caller's snapshot.
fn gate(fx: &Fx, id: &AccountId) -> GateOutcome {
    let snapshot = fx.vault_bytes(id).unwrap();
    fx.engine
        .refresh_stored(fx.cc.as_ref(), id, &snapshot)
        .unwrap()
}

/// A way to make a profile unreadable, applied to its directory.
type Break = fn(&Fx, &Path);

fn profile_unreadable(outcome: &GateOutcome) -> bool {
    matches!(outcome, GateOutcome::Transient { kind, rescued: false } if kind == "profile-unreadable")
}

#[test]
fn a_profile_in_step_leaves_the_vault_to_the_gate() {
    // P = V = S: nothing to do; the gate refreshes the vault's generation as without a profile.
    let fx = Fx::new();
    let a = due(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &fx.vault_bytes(&a).unwrap());
    fx.script_refresh(Some("rt-a2"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(sent_refresh_tokens(&fx), ["rt-a"]);
    assert_eq!(
        seed_of(&dir).seed_fp,
        fp(&fx, "rt-a"),
        "the seed stays where they agreed"
    );
}

#[test]
fn an_in_step_profile_whose_seed_lags_is_reseeded_before_the_request() {
    // P = V, S older: the seed moves to V first, so the refresh that follows leaves the profile
    // "vault moved on", not in conflict.
    let fx = Fx::new();
    let a = due(&fx);
    let dir = quiescent(&fx, &a, "rt-old", &fx.vault_bytes(&a).unwrap());
    fx.script_refresh(Some("rt-a2"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a"));
    assert_eq!(token_requests(&fx), 1);
}

#[test]
fn a_rotated_profile_is_captured_when_its_token_expires_no_later() {
    // §15.2 "Provenance": the rotated generation's access token expires exactly when the
    // vault's does, so expiry cannot tell them apart; the seed does (B.52). The gate then
    // refreshes the profile's generation, never the consumed rt-a.
    let fx = Fx::new();
    let a = due(&fx);
    let rotated = cred_at("rt-a2", expires_at(&fx.vault_bytes(&a).unwrap()));
    let dir = quiescent(&fx, &a, "rt-a", &rotated);
    fx.script_refresh(Some("rt-a3"));
    let outcome = gate(&fx, &a);
    assert!(matches!(outcome, GateOutcome::Refreshed(_)), "{outcome:?}");
    assert_eq!(sent_refresh_tokens(&fx), ["rt-a2"]);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a3"));
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a2"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a2"));
    assert_eq!(
        fs::read(dir.join(".credentials.json")).unwrap(),
        rotated,
        "the profile is never written"
    );
}

#[test]
fn a_rotated_profile_is_captured_when_its_token_expires_earlier() {
    // The vault's token is good for an hour, the profile's for half that: an expiry rule would
    // keep the vault's. The captured token is still valid, so no request is made.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let rotated = cred_at("rt-a2", fx.clock.now_ms() + 30 * 60_000);
    let dir = quiescent(&fx, &a, "rt-a", &rotated);
    let outcome = gate(&fx, &a);
    let GateOutcome::AlreadyFresh(bytes) = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(bytes, rotated);
    assert_eq!(fx.vault_bytes(&a).unwrap(), rotated);
    assert_eq!(prev_refresh_token(&fx, &a).as_deref(), Some("rt-a"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a2"));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn on_linux_a_rotated_profile_is_captured_from_its_file() {
    let fx = Fx::with_platform(Platform::Linux);
    let a = two_accounts(&fx);
    let rotated = credential("a@x.co", "rt-a2");
    let dir = quiescent(&fx, &a, "rt-a", &rotated);
    assert!(matches!(gate(&fx, &a), GateOutcome::AlreadyFresh(_)));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a2"));
}

#[test]
fn a_rotated_profile_is_still_captured_after_the_data_directory_moved() {
    // Decision 19, §12.2: the profile rotated, then moved with the data directory, so its
    // canonical path changed while its marker keeps the old spelling. Its identity and its file
    // are read where it is now, and on macOS its hashed item under the old spelling, into which
    // Claude Code had moved the file (Appendix A.3). The rotation is captured, and its token is
    // still valid, so nothing is sent.
    for platform in [Platform::MacOs, Platform::Linux] {
        let fx = Fx::with_platform(platform);
        let a = two_accounts(&fx);
        let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
        let rotated = cred_at("rt-a2", fx.clock.now_ms() + 30 * 60_000);
        // The quiescent profile, bootstrapped and rotated under the data directory's old path.
        let old = fx
            .dir
            .path()
            .join("old-data/tagteam/sessions")
            .join(a.as_str());
        let marker = fx.write_marker(&old, &a, &fx.env);
        fs::write(
            old.join(".claude.json"),
            json!({"oauthAccount": row.identity_json}).to_string(),
        )
        .unwrap();
        fx.write_seed(&old, row.login_epoch, &fp(&fx, "rt-a"));
        match platform {
            Platform::MacOs => {
                let (svc, acct) = fx.item_for_spelling(&marker.config_dir);
                fx.kc.put(&svc, &acct, &rotated);
            }
            Platform::Linux => fx.set_profile_credential(&old, &rotated),
        }
        // The move: the profile is now where this data directory puts it.
        let dir = fx.profile_dir(&a);
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        fs::rename(&old, &dir).unwrap();
        let current = fx
            .cc
            .profile_spelling(&canonical_profile_path(&dir).unwrap());
        assert_ne!(
            marker.config_dir, current,
            "{platform:?}: the spelling changed"
        );

        let outcome = gate(&fx, &a);

        let GateOutcome::AlreadyFresh(bytes) = outcome else {
            panic!("{platform:?}: {outcome:?}")
        };
        assert_eq!(bytes, rotated, "{platform:?}");
        assert_eq!(fx.vault_bytes(&a).unwrap(), rotated, "{platform:?}");
        assert_eq!(token_requests(&fx), 0, "{platform:?}");
        assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a2"), "{platform:?}");
        assert_eq!(
            ProfileMarker::read(&dir).present().map(|m| m.config_dir),
            Some(marker.config_dir.clone()),
            "{platform:?}: only M4b's next bootstrap records the new spelling"
        );
    }
}

#[test]
fn a_profile_the_vault_moved_past_is_left_alone() {
    // P = S: the vault moved on; P may be consumed and is never captured.
    let fx = Fx::new();
    let a = due(&fx);
    let dir = quiescent(&fx, &a, "rt-old", &credential("a@x.co", "rt-old"));
    fx.script_refresh(Some("rt-a2"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(sent_refresh_tokens(&fx), ["rt-a"]);
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-old"));
}

#[test]
fn a_stale_marked_profile_never_wins_over_a_replacement() {
    // Both stale-marked rows: the vault still at the seed (V = S), and both moved.
    for seed_rt in ["rt-a", "rt-old"] {
        let fx = Fx::new();
        let a = due(&fx);
        quiescent(&fx, &a, seed_rt, &credential("a@x.co", "rt-a2"));
        bump_epoch(&fx, &a);
        fx.script_refresh(Some("rt-a3"));
        assert!(
            matches!(gate(&fx, &a), GateOutcome::Refreshed(_)),
            "{seed_rt}"
        );
        assert_eq!(
            sent_refresh_tokens(&fx),
            ["rt-a"],
            "{seed_rt}: never captured"
        );
    }
}

#[test]
fn a_conflict_sends_nothing_and_changes_nothing() {
    // P ≠ V, P ≠ S, V ≠ S, not stale-marked (B.52).
    let fx = Fx::new();
    let a = due(&fx);
    let profile = credential("a@x.co", "rt-a2");
    let dir = quiescent(&fx, &a, "rt-old", &profile);
    fx.script_refresh(Some("rt-a3"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Conflict));
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-old"));
    assert_eq!(fs::read(dir.join(".credentials.json")).unwrap(), profile);
}

#[test]
fn a_profile_that_cannot_be_read_stops_the_gate() {
    // Decision 9: the seed, the marker, the identity, or the credential.
    let breaks: [(&str, Break); 6] = [
        ("seed", |_, dir| {
            fs::write(dir.join(SEED_FILE), "{").unwrap()
        }),
        ("seed link to nothing", |fx, dir| {
            fs::remove_file(dir.join(SEED_FILE)).unwrap();
            std::os::unix::fs::symlink(fx.dir.path().join("nowhere"), dir.join(SEED_FILE)).unwrap();
        }),
        ("marker", |_, dir| {
            fs::write(dir.join(MARKER_FILE), "{").unwrap()
        }),
        ("identity", |_, dir| {
            fs::remove_file(dir.join(".claude.json")).unwrap();
            fs::create_dir(dir.join(".claude.json")).unwrap();
        }),
        ("degraded credential", |fx, dir| {
            let (svc, acct) = fx.profile_item(dir);
            fx.kc.set_unreadable(&svc, &acct, true);
        }),
        ("unreadable credential", |fx, dir| {
            let (svc, acct) = fx.profile_item(dir);
            fx.kc.set_unreadable(&svc, &acct, true);
            fs::remove_file(dir.join(".credentials.json")).unwrap();
        }),
    ];
    for (what, break_it) in breaks {
        let fx = Fx::new();
        let a = due(&fx);
        let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
        break_it(&fx, &dir);
        fx.script_refresh(Some("rt-a3"));
        let outcome = gate(&fx, &a);
        assert!(profile_unreadable(&outcome), "{what}: {outcome:?}");
        assert_eq!(token_requests(&fx), 0, "{what}");
        assert_eq!(
            fx.vault_refresh_token(&a).as_deref(),
            Some("rt-a"),
            "{what}"
        );
    }
}

#[test]
fn a_profile_whose_login_drifted_is_ignored() {
    // §12.5 "Identity drift": would be a capture, but the profile is logged in as c.
    let fx = Fx::new();
    let a = due(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
    fs::write(
        dir.join(".claude.json"),
        json!({"oauthAccount": Fx::oauth_account("c@x.co")}).to_string(),
    )
    .unwrap();
    fx.script_refresh(Some("rt-a3"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(sent_refresh_tokens(&fx), ["rt-a"]);
}

/// Ways a profile's `.claude.json` names no login: no file, or no `oauthAccount` in it.
const NO_IDENTITY: [(&str, Break); 2] = [
    ("no .claude.json", |_, dir| {
        fs::remove_file(dir.join(".claude.json")).unwrap()
    }),
    ("no oauthAccount", |_, dir| {
        fs::write(dir.join(".claude.json"), "{}").unwrap()
    }),
];

#[test]
fn a_seeded_profile_without_an_identity_stops_a_capture_and_sends_nothing() {
    // Decision 9: the profile holds a rotation of the vault's generation by its provenance, but
    // nothing says the login is the account's. Ignoring it would refresh rt-a, which the
    // rotation consumed, and could quarantine a healthy lineage; capturing it would be a guess.
    for (what, strip) in NO_IDENTITY {
        let fx = Fx::new();
        let a = due(&fx);
        let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
        strip(&fx, &dir);
        fx.script_refresh(Some("rt-a3"));
        let outcome = gate(&fx, &a);
        assert!(profile_unreadable(&outcome), "{what}: {outcome:?}");
        assert_eq!(token_requests(&fx), 0, "{what}");
        assert_eq!(
            fx.vault_refresh_token(&a).as_deref(),
            Some("rt-a"),
            "{what}"
        );
        assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a"), "{what}");
    }
}

#[test]
fn a_seeded_profile_without_an_identity_that_holds_no_rotation_leaves_the_vault_to_the_gate() {
    // The identity matters only to a capture. In step (a bootstrap that stopped before it seeded
    // `.claude.json`), moved past by the vault, or logged out of: nothing in the profile can be
    // a rotation of the vault's generation, so the gate refreshes that generation as without a
    // profile, and the next launch bootstraps the profile again.
    let rows: [(&str, &str, Option<&str>); 3] = [
        ("in step", "rt-a", Some("rt-a")),
        ("vault moved on", "rt-old", Some("rt-old")),
        ("logged out", "rt-a", None),
    ];
    for (row, seed_rt, held_rt) in rows {
        for (what, strip) in NO_IDENTITY {
            let fx = Fx::new();
            let a = due(&fx);
            let held = match held_rt {
                Some("rt-a") | None => fx.vault_bytes(&a).unwrap(),
                Some(rt) => credential("a@x.co", rt),
            };
            let dir = quiescent(&fx, &a, seed_rt, &held);
            if held_rt.is_none() {
                fs::remove_file(dir.join(".credentials.json")).unwrap();
            }
            strip(&fx, &dir);
            fx.script_refresh(Some("rt-a2"));
            let outcome = gate(&fx, &a);
            assert!(
                matches!(outcome, GateOutcome::Refreshed(_)),
                "{row}, {what}: {outcome:?}"
            );
            assert_eq!(sent_refresh_tokens(&fx), ["rt-a"], "{row}, {what}");
        }
    }
}

#[test]
fn a_profile_never_bootstrapped_is_ignored() {
    let fx = Fx::new();
    let a = due(&fx);
    let dir = fx.make_profile(&a);
    fx.set_profile_credential(&dir, &credential("a@x.co", "rt-a2"));
    fx.script_refresh(Some("rt-a3"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(sent_refresh_tokens(&fx), ["rt-a"]);
}

#[test]
fn a_profile_credential_without_a_refresh_token_is_never_captured() {
    // §6.2: never a credential without a refresh token over one with it.
    let fx = Fx::new();
    let a = due(&fx);
    let access_only =
        json!({"claudeAiOauth": {"accessToken": "at-x", "expiresAt": 1_790_003_600_000i64}});
    quiescent(&fx, &a, "rt-a", access_only.to_string().as_bytes());
    fx.script_refresh(Some("rt-a2"));
    assert!(matches!(gate(&fx, &a), GateOutcome::Refreshed(_)));
    assert_eq!(sent_refresh_tokens(&fx), ["rt-a"]);
}

#[test]
fn a_running_profile_is_never_captured() {
    // §6.2: a capture from a profile requires it to be quiescent; the gate stops at step 2.
    let fx = Fx::new();
    let a = due(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
    let _held = fx.hold_reservation(&dir);
    assert!(matches!(
        gate(&fx, &a),
        GateOutcome::Owned(tagteam_engine::refresh::OwnedBy::Session)
    ));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a"));
}

// §9.2: lazy capture and the conflict refusal in `switch`.

#[test]
fn a_switch_activates_a_rotated_profiles_generation() {
    // §12.5 "Lazy capture" at the switch: a's token is not due, so no gate runs. Freshening
    // adopts the profile's rotation under a's account lock, before it judges whether the token
    // is due, and the switch activates it, never the consumed rt-a.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a2"));
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_rotated_profile_whose_access_token_is_due_is_freshened_before_activation() {
    // §7.2 and §9.2: the vault's rt-a is not due, but the profile's rotation rt-a2 expires
    // within the freshen window. Lazy capture runs before the freshen decision, so the gate
    // refreshes rt-a2 and the switch activates its successor, never an access token about
    // to expire.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let soon = fx.clock.now_ms() + 60_000;
    quiescent(&fx, &a, "rt-a", &cred_at("rt-a2", soon));
    fx.script_refresh(Some("rt-a3"));
    fx.switch_to(&a, false).unwrap();
    assert_eq!(token_requests(&fx), 1);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a3"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a3"));
}

#[cfg(feature = "test-hooks")]
#[test]
fn a_pick_made_under_the_mutation_lock_activates_its_profiles_rotation() {
    // §9.2 lazy capture in the transaction itself. A pick made again under the mutation lock
    // is never freshened (§4.3: no network there), so only the transaction's own capture keeps
    // the consumed rt-a from being activated. The rotation first picks x; a session takes x
    // once the switch has planned, so under the locks it plans again and picks a, whose
    // quiescent profile rotated to rt-a2.
    use std::sync::Mutex;
    use tagteam_provider::FlockGuard;
    use tagteam_provider::profile::LAUNCH_DIR;

    let fx = Fx::new();
    let x = fx.add("x@x.co", "rt-x");
    let a = fx.add("a@x.co", "rt-a");
    fx.add("c@x.co", "rt-c"); // live: the walk starts after it, at x
    let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
    let launch = fx.make_profile(&x).join(LAUNCH_DIR).join("4242.lock");
    let held = Mutex::new(None);
    fx.engine.on_point(
        "planned",
        Box::new(move || {
            let mut held = held.lock().unwrap();
            if held.is_none() {
                *held = FlockGuard::try_lock(&launch).unwrap();
            }
        }),
    );

    let out = fx.engine.switch(fx.rotation_request(false)).unwrap();

    assert_eq!(out.to.map(|t| t.id), Some(a.clone()));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a2"));
    assert!(sent_refresh_tokens(&fx).is_empty(), "nothing is sent");
}

#[cfg(feature = "test-hooks")]
#[test]
fn a_replacement_between_planning_and_freshening_stale_marks_the_profile() {
    // The plan read a's row before an explicit replacement landed. The replacement installs
    // the seed's own generation again, so the vault equals the seed while the profile holds a
    // rotation: judged with the planning row's epoch, that would capture the profile over the
    // replacement. Freshen reads the row again under the account lock, sees the moved epoch,
    // and the replacement wins (§12.5).
    use std::sync::atomic::{AtomicBool, Ordering};
    use tagteam_engine::store::LoginMeta;
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
    let store = fx.engine.store().unwrap();
    let cc = fx.cc.clone();
    let id = a.clone();
    let seed_fp = fp(&fx, "rt-a");
    let once = AtomicBool::new(false);
    fx.engine.on_point(
        "freshen-before-lock",
        Box::new(move || {
            if once.swap(true, Ordering::SeqCst) {
                return;
            }
            let row = store.account(&id).unwrap().unwrap();
            let identity = cc.parse_identity(&row.identity_json).unwrap();
            let meta = LoginMeta {
                identity_key: &row.identity_key,
                identity: &identity,
                kind: &row.kind,
                login_expires_at: row.login_expires_at,
                from_live: false,
            };
            // The vault already holds rt-a: the replacement's write leaves its bytes as they
            // are, and only the epoch moves.
            store
                .begin_replacement(&id, &seed_fp, &meta, false)
                .unwrap();
            store.finish_replacement(&id, 0).unwrap();
        }),
    );

    fx.switch_to(&a, false).unwrap();

    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(seed_of(&dir).seed_fp, fp(&fx, "rt-a"), "never captured");
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_switch_to_a_profile_the_vault_moved_past_activates_the_vault() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    quiescent(&fx, &a, "rt-old", &credential("a@x.co", "rt-old"));
    fx.switch_to(&a, false).unwrap();
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
}

#[test]
fn a_switch_to_a_conflicting_profile_refuses_with_or_without_force() {
    for force in [false, true] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        quiescent(&fx, &a, "rt-old", &credential("a@x.co", "rt-a2"));
        let err = fx.switch_to(&a, force).unwrap_err();
        assert_eq!(err.kind(), "profile-conflict", "force {force}: {err}");
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
        assert!(journal(&fx).is_none());
    }
}

#[test]
fn a_due_target_with_a_conflicting_profile_refuses_before_any_request() {
    // §7.2's table: freshening meets the gate's `Conflict`.
    let fx = Fx::new();
    let a = due(&fx);
    quiescent(&fx, &a, "rt-old", &credential("a@x.co", "rt-a2"));
    fx.script_refresh(Some("rt-a3"));
    let err = fx.switch_to(&a, false).unwrap_err();
    assert_eq!(err.kind(), "profile-conflict", "{err}");
    assert_eq!(token_requests(&fx), 0);
}

#[test]
fn a_switch_to_an_account_whose_profile_cannot_be_read_refuses() {
    // Decision 9, due or not: the switch refuses before any request, at the lazy capture that
    // precedes the freshen decision.
    for due_now in [false, true] {
        let fx = Fx::new();
        let a = if due_now { due(&fx) } else { two_accounts(&fx) };
        let dir = quiescent(&fx, &a, "rt-a", &credential("a@x.co", "rt-a2"));
        fs::write(dir.join(SEED_FILE), "{").unwrap();
        let err = fx.switch_to(&a, false).unwrap_err();
        assert_eq!(err.kind(), "unreadable", "due {due_now}: {err}");
        assert!(err.to_string().contains("position 1"), "{err}");
        assert_eq!(token_requests(&fx), 0, "due {due_now}");
        assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
    }
}
