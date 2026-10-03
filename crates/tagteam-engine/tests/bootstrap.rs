//! §12.3 steps 2–8: a quiescent profile's bootstrap from the vault, and its validation. Driven
//! through the `test-hooks` seam `Engine::bootstrap_quiescent`, which runs what a quiescent
//! launch runs between its locks and its reservation, provenance aside (Task 10 puts it
//! first, so a rotation is captured before any bootstrap).
#![cfg(feature = "test-hooks")]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::{
    Fx, auth_helper, auth_logged_out, auth_reply, auth_status, credential, two_accounts, vault_fp,
};
use serde_json::{Value, json};
use tagteam_cc::live::Platform;
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::bootstrap::Trigger;
use tagteam_engine::store::AccountRow;
use tagteam_provider::process::{Captured, ProcessSpawner, SpawnSpec};
use tagteam_provider::profile::{MARKER_FILE, ProfileMarker, Seed};
use tagteam_provider::splice::get_top_level;
use tagteam_provider::{Cancel, Read};

fn row(fx: &Fx, id: &AccountId) -> AccountRow {
    fx.engine.store().unwrap().account(id).unwrap().unwrap()
}

fn seed(profile: &Path) -> Seed {
    match Seed::read(profile) {
        Read::Present(s) => s,
        other => panic!("no seed in {}: {other:?}", profile.display()),
    }
}

fn marker(profile: &Path) -> ProfileMarker {
    match ProfileMarker::read(profile) {
        Read::Present(m) => m,
        other => panic!("no marker in {}: {other:?}", profile.display()),
    }
}

/// What Task 11's per-launch check records for an `invalid` login (§12.3).
fn set_needs_bootstrap(profile: &Path) {
    Seed {
        needs_bootstrap: true,
        ..seed(profile)
    }
    .write(profile)
    .unwrap();
}

fn bootstrap(fx: &Fx, id: &AccountId) -> Option<Trigger> {
    fx.engine
        .bootstrap_quiescent(id, &fx.work_dir("app"))
        .unwrap()
}

/// `id`'s profile bootstrapped from the vault and validated, as a first launch leaves it.
fn bootstrapped(fx: &Fx, id: &AccountId, email: &str) -> PathBuf {
    let profile = fx.profile_dir(id);
    fx.script_valid(&profile, email);
    assert_eq!(bootstrap(fx, id), Some(Trigger::Missing));
    profile
}

/// The spelling `id`'s profile had before the data directory moved (§12.2, Decision 22): a
/// path that is gone, which a moved profile's marker still records.
fn moved_from(fx: &Fx, id: &AccountId) -> String {
    let old = fx.dir.path().join("moved-from/sessions").join(id.as_str());
    old.to_str().unwrap().to_owned()
}

/// A login check's reply, made for the spelling of the profile it checks.
type Reply = fn(&str) -> Captured;

/// A login check's exit code and `--json` body, made for the spelling of the profile it checks.
type Status = fn(&str) -> (i32, Value);

/// A login check a SIGTERM lands in (§12.5 "Signals"): the token is set while it runs, and the
/// spawner kills its group.
struct SignalledCheck;

impl ProcessSpawner for SignalledCheck {
    fn run_captured(&self, _spec: &SpawnSpec, _timeout: Duration, cancel: &Cancel) -> Captured {
        cancel.request(15);
        Captured::Interrupted(15)
    }
}

#[test]
fn a_missing_profile_is_bootstrapped_from_the_vault_seeded_and_validated() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = fx.profile_dir(&a);
    let cwd = fx.work_dir("app");
    let spelling = fx.spelling_for(&profile);
    fx.script_valid(&profile, "a@x.co");

    let trigger = fx.engine.bootstrap_quiescent(&a, &cwd).unwrap();

    assert_eq!(trigger, Some(Trigger::Missing));
    assert_eq!(
        marker(&profile).config_dir,
        spelling,
        "§12.2: one recorded spelling"
    );
    assert_eq!(
        seed(&profile),
        Seed {
            login_epoch: row(&fx, &a).login_epoch,
            seed_fp: vault_fp(&fx, &a),
            needs_bootstrap: false,
        },
        "step 6"
    );
    let held = fx.held_credential(&profile).unwrap();
    assert_eq!(held["claudeAiOauth"]["refreshToken"], "rt-a");
    assert!(
        held.get("mcpOAuth").is_none(),
        "step 4: a new profile starts with no machine-shared keys"
    );
    let mode = fs::metadata(profile.join(".credentials.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "§5, B.33");
    let (svc, acct) = fx.profile_item(&profile);
    assert_eq!(
        fx.kc.get(&svc, &acct),
        None,
        "step 5: never written, verified gone"
    );
    let config = fs::read(profile.join(".claude.json")).unwrap();
    assert_eq!(
        get_top_level(&config, "hasCompletedOnboarding").unwrap(),
        Some(json!(true)),
        "step 7 (§12.4)"
    );
    let specs = fx.spawner.specs();
    assert_eq!(specs.len(), 1, "step 8: validated once");
    assert_eq!(specs[0].cwd.as_deref(), Some(cwd.as_path()));
    assert!(
        specs[0]
            .set
            .contains(&("CLAUDE_CONFIG_DIR".into(), spelling.clone().into())),
        "in the session's environment: {:?}",
        specs[0]
    );
}

#[test]
fn a_profile_in_step_is_only_seeded() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    let seeded = seed(&profile);

    assert_eq!(bootstrap(&fx, &a), None);

    assert_eq!(seed(&profile), seeded);
    assert_eq!(
        fx.spawner.specs().len(),
        1,
        "no validation without a bootstrap"
    );
    assert!(
        profile.join(".tagteam-baseline.json").exists(),
        "seeded (§12.4)"
    );
}

#[test]
fn a_profile_its_login_check_found_invalid_is_bootstrapped_again() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    set_needs_bootstrap(&profile);
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::Invalid));

    assert!(!seed(&profile).needs_bootstrap);
    assert_eq!(fx.spawner.specs().len(), 2);
}

#[test]
fn a_stale_marked_profile_has_its_credential_displaced_then_is_bootstrapped() {
    // §12.3 step 3: it may hold a live generation of the login the replacement superseded.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    fx.rotate_profile(&profile, "rt-a-session");
    fx.land_replacement(&a, "rt-a-new");
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::StaleMarked));

    let displaced: Vec<Value> = fx
        .displaced()
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    assert_eq!(displaced.len(), 1, "{displaced:?}");
    assert_eq!(
        displaced[0]["claudeAiOauth"]["refreshToken"],
        "rt-a-session"
    );
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-new"));
    assert_eq!(seed(&profile).login_epoch, row(&fx, &a).login_epoch);
    let (svc, acct) = fx.profile_item(&profile);
    assert_eq!(fx.kc.get(&svc, &acct), None);
}

#[test]
fn a_profile_the_vault_moved_past_is_bootstrapped_without_a_copy_of_its_old_generation() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    fx.put_vault(&a, &credential("a@x.co", "rt-a-2"));
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::OtherCredential));

    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
    assert_eq!(seed(&profile).seed_fp, vault_fp(&fx, &a));
    assert!(
        fx.displaced().is_empty(),
        "this account's older generation, not displaced"
    );
}

#[test]
fn a_profile_without_a_credential_is_bootstrapped() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    fs::remove_file(profile.join(".credentials.json")).unwrap();
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::OtherCredential));

    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a"));
}

#[test]
fn another_login_in_the_profile_is_displaced_before_the_bootstrap_overwrites_it() {
    // §12.5 "Identity drift", B.5: bytes that are not ours are displaced before being
    // overwritten.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    // Someone ran `/login` as c@x.co inside a session.
    fx.set_profile_identity(&profile, "c@x.co");
    fx.cc_writes_profile(&profile, |v| *v = Fx::credential_json("c@x.co", "rt-c"));
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::OtherCredential));

    let displaced: Vec<Value> = fx
        .displaced()
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    assert_eq!(displaced.len(), 1, "{displaced:?}");
    assert_eq!(displaced[0]["claudeAiOauth"]["refreshToken"], "rt-c");
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a"));
    let config = fs::read(profile.join(".claude.json")).unwrap();
    let identity = get_top_level(&config, "oauthAccount").unwrap().unwrap();
    assert_eq!(
        identity["emailAddress"], "a@x.co",
        "re-seeded with the account's login"
    );
}

#[test]
fn a_profile_recorded_under_another_spelling_takes_the_new_one_and_both_items_go() {
    // §12.2 "One spelling", §12.3 steps 5 and 6. A trailing slash changes the hash
    // (Appendix A.2), as a moved data directory changes the whole path.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    let current = fx.spelling_for(&profile);
    let old = format!("{current}/");
    ProfileMarker {
        config_dir: old.clone(),
        ..marker(&profile)
    }
    .write(&profile)
    .unwrap();
    let held = fs::read(profile.join(".credentials.json")).unwrap();
    let (old_item, acct) = fx.item_for_spelling(&old);
    let (new_item, _) = fx.item_for_spelling(&current);
    assert_ne!(old_item, new_item);
    fx.kc.put(&old_item, &acct, &held);
    fx.kc.put(&new_item, &acct, &held);
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::SpellingChanged));

    assert_eq!(
        fx.kc.get(&old_item, &acct),
        None,
        "the old spelling's item, verified gone"
    );
    assert_eq!(fx.kc.get(&new_item, &acct), None, "and the current one's");
    assert_eq!(
        marker(&profile).config_dir,
        current,
        "recorded after the deletion"
    );
    let last = fx.spawner.specs().pop().unwrap();
    assert!(
        last.set
            .contains(&("CLAUDE_CONFIG_DIR".into(), current.clone().into())),
        "{last:?}"
    );
}

#[test]
fn a_moved_profile_whose_old_item_holds_its_only_credential_keeps_its_machine_shared_keys() {
    // Decision 22, §12.3 steps 2, 4 and 5. The data directory moved, so the marker's spelling
    // names a path that is gone. Claude Code had moved the file into that spelling's item
    // (Appendix A.3): the only copy of the credential, and of the MCP token it minted.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    let current = fx.spelling_for(&profile);
    let old = moved_from(&fx, &a);
    let file = profile.join(".credentials.json");
    let mut held: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    held["mcpOAuth"] = json!({"srv": {"token": "minted-in-session"}});
    let (old_item, acct) = fx.item_for_spelling(&old);
    fx.kc.put(&old_item, &acct, held.to_string().as_bytes());
    fs::remove_file(&file).unwrap();
    ProfileMarker {
        config_dir: old.clone(),
        ..marker(&profile)
    }
    .write(&profile)
    .unwrap();
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::SpellingChanged));

    let written: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(
        written["mcpOAuth"]["srv"]["token"], "minted-in-session",
        "step 4 kept the machine-shared keys only the old item held"
    );
    assert_eq!(written["claudeAiOauth"]["refreshToken"], "rt-a");
    assert_eq!(
        fx.kc.get(&old_item, &acct),
        None,
        "step 5: the old spelling's item, deleted once the file holds its keys"
    );
    assert_eq!(marker(&profile).config_dir, current);
}

#[test]
fn a_moved_profile_s_stale_credential_is_read_where_the_profile_is_and_displaced() {
    // Decision 22, §12.3 steps 2 and 3: the file is read in the actual directory, not under the
    // old spelling's path, so a stale-marked credential there is saved before step 4
    // overwrites it.
    let fx = Fx::with_platform(Platform::Linux);
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    fx.rotate_profile(&profile, "rt-a-session");
    fx.land_replacement(&a, "rt-a-new");
    ProfileMarker {
        config_dir: moved_from(&fx, &a),
        ..marker(&profile)
    }
    .write(&profile)
    .unwrap();
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::StaleMarked));

    let displaced: Vec<Value> = fx
        .displaced()
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    assert_eq!(displaced.len(), 1, "{displaced:?}");
    assert_eq!(
        displaced[0]["claudeAiOauth"]["refreshToken"],
        "rt-a-session"
    );
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-new"));
    assert_eq!(marker(&profile).config_dir, fx.spelling_for(&profile));
}

#[test]
fn a_bootstrap_stopped_between_the_file_and_the_item_is_redone_with_the_same_machine_shared_keys() {
    // §12.3 step 5: the file is written before the item goes, so the item's machine-shared keys
    // are on disk when it does. Stopped between the two, the old item stays authoritative and
    // the seed unchanged, so the next bootstrap reads the same keys from the same item.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    // Claude Code in a session authenticated an MCP server, which moved the credential into the
    // profile's item.
    fx.cc_writes_profile(&profile, |v| {
        v["mcpOAuth"] = json!({"srv": {"token": "minted-in-session"}})
    });
    fx.put_vault(&a, &credential("a@x.co", "rt-a-2"));
    let (svc, acct) = fx.profile_item(&profile);
    let seeded = seed(&profile);
    fx.engine.fail_at(Some("bootstrap-after-credential-write"));

    let err = fx
        .engine
        .bootstrap_quiescent(&a, &fx.work_dir("app"))
        .unwrap_err();

    fx.engine.fail_at(None);
    assert!(err.to_string().contains("injected failure"), "{err}");
    let item: Value = serde_json::from_slice(&fx.kc.get(&svc, &acct).unwrap()).unwrap();
    assert_eq!(
        item["claudeAiOauth"]["refreshToken"], "rt-a",
        "the old item stays"
    );
    let file: Value =
        serde_json::from_slice(&fs::read(profile.join(".credentials.json")).unwrap()).unwrap();
    assert_eq!(
        file["mcpOAuth"]["srv"]["token"], "minted-in-session",
        "already on disk"
    );
    assert_eq!(seed(&profile), seeded, "nothing recorded");
    assert_eq!(fx.spawner.specs().len(), 1, "never validated");

    fx.script_valid(&profile, "a@x.co");
    assert_eq!(bootstrap(&fx, &a), Some(Trigger::OtherCredential));

    assert_eq!(fx.kc.get(&svc, &acct), None, "verified gone");
    let held = fx.held_credential(&profile).unwrap();
    assert_eq!(held["claudeAiOauth"]["refreshToken"], "rt-a-2");
    assert_eq!(held["mcpOAuth"]["srv"]["token"], "minted-in-session");
}

#[test]
fn an_item_that_cannot_be_verified_gone_aborts_and_the_next_bootstrap_retries() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    fx.cc_writes_profile(&profile, |_| {});
    fx.put_vault(&a, &credential("a@x.co", "rt-a-2"));
    let (svc, acct) = fx.profile_item(&profile);
    let seeded = seed(&profile);
    fx.kc.set_fail_delete(&svc, true);

    let err = fx
        .engine
        .bootstrap_quiescent(&a, &fx.work_dir("app"))
        .unwrap_err();

    fx.kc.set_fail_delete(&svc, false);
    assert!(
        err.to_string().contains("could not be verified gone"),
        "{err}"
    );
    assert!(fx.kc.get(&svc, &acct).is_some());
    assert_eq!(
        seed(&profile),
        seeded,
        "unrecorded, so the next launch bootstraps again"
    );

    fx.script_valid(&profile, "a@x.co");
    assert_eq!(bootstrap(&fx, &a), Some(Trigger::OtherCredential));
    assert_eq!(fx.kc.get(&svc, &acct), None);
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
}

#[test]
fn an_unreadable_or_degraded_profile_credential_aborts_without_writing() {
    // §12.3 step 2: tagteam never overwrites a credential it could not read.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    set_needs_bootstrap(&profile);
    let (svc, acct) = fx.profile_item(&profile);
    fx.kc.set_unreadable(&svc, &acct, true);
    let file = fs::read(profile.join(".credentials.json")).unwrap();

    // The file covers the unreadable item: degraded.
    let err = fx
        .engine
        .bootstrap_quiescent(&a, &fx.work_dir("app"))
        .unwrap_err();
    assert_eq!(err.kind(), "unreadable", "{err}");
    assert_eq!(fs::read(profile.join(".credentials.json")).unwrap(), file);

    // Nothing covers it: unreadable.
    fs::remove_file(profile.join(".credentials.json")).unwrap();
    let err = fx
        .engine
        .bootstrap_quiescent(&a, &fx.work_dir("app"))
        .unwrap_err();
    assert_eq!(err.kind(), "unreadable", "{err}");
    assert!(!profile.join(".credentials.json").exists());

    assert!(seed(&profile).needs_bootstrap, "still due");
    assert_eq!(
        fx.spawner.specs().len(),
        1,
        "only the first bootstrap validated"
    );
}

#[test]
fn a_pending_rescue_is_adopted_before_the_profile_is_composed() {
    // §6.2: a rescue consumed the vault's generation; bootstrapping the vault alone would hand
    // the session a spent refresh token.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = bootstrapped(&fx, &a, "a@x.co");
    fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    set_needs_bootstrap(&profile);
    fx.script_valid(&profile, "a@x.co");

    assert_eq!(bootstrap(&fx, &a), Some(Trigger::Invalid));

    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
    assert_eq!(seed(&profile).seed_fp, vault_fp(&fx, &a));
}

#[test]
fn an_invalid_login_at_bootstrap_deletes_the_profile_and_refuses() {
    // §12.3 step 8, B.29: only `invalid` deletes a profile; the next `run` starts afresh.
    let cases: [Status; 2] = [
        |s| (1, auth_logged_out(s)),
        |s| (0, auth_status(s, "c@x.co")),
    ];
    for case in cases {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let profile = fx.profile_dir(&a);
        let (code, body) = case(&fx.spelling_for(&profile));
        fx.script_auth(code, &body);

        let err = fx
            .engine
            .bootstrap_quiescent(&a, &fx.work_dir("app"))
            .unwrap_err();

        assert_eq!(err.kind(), "login-invalid", "{err}");
        assert!(!profile.exists(), "the profile is deleted: {err}");
    }
}

#[test]
fn an_invalid_login_over_a_profile_split_meanwhile_keeps_the_profile_and_refuses_as_the_split() {
    // R9.1, §12.2: real history is never deleted silently, so B.29's deletion checks for a
    // split first, as `remove` does. A must-share copy appears between the sync, which would
    // have refused it, and the login check.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = fx.profile_dir(&a);
    fx.script_auth(1, &auth_logged_out(&fx.spelling_for(&profile)));
    let projects = profile.join("projects");
    let copy = projects.clone();
    fx.engine.on_point(
        "bootstrap-after-credential-write",
        Box::new(move || {
            fs::remove_file(&copy).unwrap();
            fs::create_dir(&copy).unwrap();
            fs::write(copy.join("only-here.jsonl"), "{}\n").unwrap();
        }),
    );

    let err = fx
        .engine
        .bootstrap_quiescent(&a, &fx.work_dir("app"))
        .unwrap_err();

    assert_eq!(err.kind(), "profile-split", "{err}");
    assert!(err.to_string().contains("real copy"), "{err}");
    assert_eq!(
        fs::read(projects.join("only-here.jsonl")).unwrap(),
        b"{}\n",
        "the copy is kept"
    );
    assert!(profile.join(MARKER_FILE).exists(), "the profile is kept");
}

#[test]
fn an_overridden_drifted_unknown_or_unreachable_login_refuses_and_keeps_the_profile() {
    // §12.3 step 8's table, B.62.
    let rows: [(Reply, &str); 5] = [
        (
            |s| auth_reply(0, auth_helper(s).to_string().as_bytes()),
            "login-overridden",
        ),
        (
            |_| {
                auth_reply(
                    0,
                    auth_status("/somewhere/else", "a@x.co")
                        .to_string()
                        .as_bytes(),
                )
            },
            "login-drifted",
        ),
        (|_| Captured::TimedOut, "login-unknown"),
        (|_| auth_reply(0, b"not json"), "login-unknown"),
        (
            |_| Captured::SpawnFailed("No such file or directory".into()),
            "launch-unreachable",
        ),
    ];
    for (reply, kind) in rows {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let profile = fx.profile_dir(&a);
        fx.spawner.push(reply(&fx.spelling_for(&profile)));

        let err = fx
            .engine
            .bootstrap_quiescent(&a, &fx.work_dir("app"))
            .unwrap_err();

        assert_eq!(err.kind(), kind, "{err}");
        assert!(
            profile.join(MARKER_FILE).exists(),
            "{kind}: the profile is kept"
        );
        assert_eq!(seed(&profile).seed_fp, vault_fp(&fx, &a), "{kind}");
    }
}

#[test]
fn a_signal_during_the_login_check_interrupts_the_launch_and_keeps_the_profile() {
    // §12.5 "Signals": the check's wait is a cancellation point, read through the token, never
    // through the reply's text.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = fx.profile_dir(&a);
    let engine = fx.engine_with_spawner(Arc::new(SignalledCheck));

    let err = engine
        .bootstrap_quiescent(&a, &fx.work_dir("app"))
        .unwrap_err();

    assert!(matches!(err, EngineError::Interrupted(15)), "{err:?}");
    assert!(profile.join(MARKER_FILE).exists(), "the profile is kept");
    assert_eq!(seed(&profile).seed_fp, vault_fp(&fx, &a));

    // A reply that only reads as interrupted, with no signal recorded, is a login that could
    // not be confirmed.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.spawner.push(Captured::Interrupted(15));

    let err = fx
        .engine
        .bootstrap_quiescent(&a, &fx.work_dir("app"))
        .unwrap_err();

    assert_eq!(err.kind(), "login-unknown", "{err}");
}

#[test]
fn a_fresh_machine_gets_projects_and_history_created_empty_and_shared() {
    // Review Focus 5, first half: §3's create-only row, §12.2 "Must-share entries", B.43.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let claude = fx.env.home.join(".claude");
    fs::remove_dir_all(claude.join("projects")).unwrap();
    fs::remove_file(claude.join("history.jsonl")).unwrap();

    let profile = bootstrapped(&fx, &a, "a@x.co");

    assert!(
        fs::read_dir(claude.join("projects"))
            .unwrap()
            .next()
            .is_none(),
        "created empty"
    );
    assert_eq!(fs::read(claude.join("history.jsonl")).unwrap(), b"");
    for name in ["projects", "history.jsonl"] {
        assert_eq!(
            fs::read_link(profile.join(name)).unwrap(),
            fs::canonicalize(claude.join(name)).unwrap(),
            "{name} is shared"
        );
    }
}
