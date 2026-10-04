//! §12.5 "Launch": a session's start under `MutationGuard` and the account lock, what it
//! decides again under them, and its reservation; then the per-launch login check (§12.3) and
//! the exit handling (§12.5 "When the child exits").

mod common;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use common::{
    Fx, auth_helper, auth_logged_out, auth_reply, auth_status, capture_logs, claude_bin,
    credential, dir_tree, mutation_lock_free, splice_config_key, token_requests, two_accounts,
    vault_fp,
};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::launch::{LaunchEnd, Launched};
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::store::AccountRow;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::flock::{FlockGuard, LockProbe, probe_lock};
use tagteam_provider::http::{HttpError, Method};
use tagteam_provider::process::Captured;
use tagteam_provider::profile::{
    LAUNCH_DIR, MARKER_FILE, ProfileMarker, SEED_FILE, Seed, launch_reservations,
};
use tagteam_provider::splice::{get_top_level, remove_top_level};
use tagteam_provider::{Keychain, Read};

/// The row `plan_run` hands `launch` (Task 8), as it stands.
fn row(fx: &Fx, id: &AccountId) -> AccountRow {
    fx.engine.store().unwrap().account(id).unwrap().unwrap()
}

fn refused(result: Result<Launched, EngineError>) -> EngineError {
    match result {
        Ok(launched) => panic!("launched position {}", launched.account.position),
        Err(e) => e,
    }
}

fn seed(profile: &Path) -> Seed {
    match Seed::read(profile) {
        Read::Present(s) => s,
        other => panic!("no seed in {}: {other:?}", profile.display()),
    }
}

/// How many reservations in `profile` are live (§12.5: held).
fn live_reservations(profile: &Path) -> usize {
    match launch_reservations(profile) {
        Read::Present(found) => found
            .iter()
            .filter(|(_, probe)| *probe == LockProbe::Held)
            .count(),
        other => panic!("{other:?}"),
    }
}

/// One top-level key of the JSON file at `path`, `Null` when absent.
fn config_key(path: &Path, key: &str) -> Value {
    get_top_level(&fs::read(path).unwrap(), key)
        .unwrap()
        .unwrap_or(Value::Null)
}

/// A session visiting a new project: one more entry in the profile's `projects` (§12.4).
fn session_adds_project(profile: &Path, project: &str) {
    let config = profile.join(".claude.json");
    let mut projects = config_key(&config, "projects");
    projects[project] = json!({"allowedTools": ["Bash"]});
    splice_config_key(&config, "projects", &projects);
}

/// The fixture's home made unwritable (0500), so nothing can be created directly in it: the
/// merge-back's `~/.claude.json.lock` and its temporary file. Everything below it, tagteam's
/// data dir included, stays writable. Needs a non-root user.
fn block_home(fx: &Fx) {
    fs::set_permissions(&fx.env.home, fs::Permissions::from_mode(0o500)).unwrap();
}

fn unblock_home(fx: &Fx) {
    fs::set_permissions(&fx.env.home, fs::Permissions::from_mode(0o700)).unwrap();
}

/// The links in `profile`, by name, and where each points.
fn links(profile: &Path) -> BTreeMap<String, PathBuf> {
    fs::read_dir(profile)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|path| fs::symlink_metadata(path).unwrap().file_type().is_symlink())
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, fs::read_link(&path).unwrap())
        })
        .collect()
}

/// `id`'s first launch, which bootstraps (with a `valid` check queued) and seeds.
fn first_launch(fx: &Fx, id: &AccountId, email: &str) -> Launched {
    fx.script_valid(&fx.profile_dir(id), email);
    let launched = fx
        .engine
        .launch(&row(fx, id), claude_bin(), &fx.work_dir("app"))
        .unwrap();
    assert!(launched.bootstrapped);
    launched
}

/// A session of `id` whose `tagteam` was killed while `claude` ran (Review Focus 1): no exit
/// handling, so its reservation is left dead and its baseline unmerged. Returns the profile.
fn killed_session(fx: &Fx, id: &AccountId, email: &str) -> PathBuf {
    let launched = first_launch(fx, id, email);
    let profile = launched.profile.clone();
    drop(launched);
    profile
}

/// A session of `id` that ran and exited cleanly, its exit handling done. Returns the profile.
fn finished_session(fx: &Fx, id: &AccountId, email: &str) -> PathBuf {
    let launched = first_launch(fx, id, email);
    let profile = launched.profile.clone();
    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));
    assert!(notices.is_empty(), "{notices:?}");
    profile
}

#[test]
fn a_first_launch_bootstraps_validates_and_reserves_then_releases_its_locks() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = fx.profile_dir(&a);
    let cwd = fx.work_dir("app");
    let spelling = fx.spelling_for(&profile);
    fx.script_valid(&profile, "a@x.co");

    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();

    assert!(launched.bootstrapped);
    assert_eq!(launched.account.id, a);
    assert_eq!(launched.profile, profile);
    assert_eq!(launched.spelling, spelling);
    assert!(
        launched.env.set.contains(&(
            OsString::from("CLAUDE_CONFIG_DIR"),
            OsString::from(&spelling)
        )),
        "{:?}",
        launched.env
    );
    assert!(launched.warnings.is_empty(), "{:?}", launched.warnings);
    assert_eq!(
        launched.reservation.path().parent(),
        Some(profile.join(LAUNCH_DIR).as_path())
    );
    assert_eq!(
        probe_lock(launched.reservation.path()).unwrap(),
        LockProbe::Held
    );
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a"));
    assert!(
        profile.join(".tagteam-baseline.json").exists(),
        "seeded (§12.4)"
    );
    let specs = fx.spawner.specs();
    assert_eq!(specs.len(), 1, "validated once, under the locks");
    assert_eq!(
        specs[0].program,
        claude_bin(),
        "the launch command plan_run resolved (Decision 20)"
    );
    assert!(mutation_lock_free(&fx.env), "step 5: released");
    assert!(AccountLock::try_acquire(&fx.env, &a).unwrap().is_some());
}

#[test]
fn a_launch_into_a_profile_in_step_only_seeds_it() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();

    assert!(
        !launched.bootstrapped,
        "Task 11's per-launch check runs instead"
    );
    assert_eq!(fx.spawner.specs().len(), 1, "no validation under the locks");
    assert!(
        profile.join(".tagteam-baseline.json").exists(),
        "seeded again"
    );
    assert_eq!(live_reservations(&profile), 1);
}

#[test]
fn a_vault_credential_about_to_expire_is_refreshed_first_and_the_profile_starts_from_it() {
    // §12.3 step 1.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));

    let launched = first_launch(&fx, &a, "a@x.co");

    assert_eq!(token_requests(&fx), 1);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert_eq!(
        fx.held_refresh_token(&launched.profile).as_deref(),
        Some("rt-a-2")
    );
}

#[test]
fn a_refresh_whose_successor_reached_only_rescue_aborts_the_launch_before_anything_is_written() {
    // §12.3 step 1: the vault's generation is consumed. The refusal is carried across the locks
    // and refuses this quiescent launch there (fix round 2), before the profile is created.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.script_refresh(Some("rt-a-2"));
    fx.kc.set_fail_write(SERVICE, true);

    let err = refused(
        fx.engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")),
    );

    fx.kc.set_fail_write(SERVICE, false);
    assert_eq!(err.kind(), "rescue-pending", "{err}");
    assert!(!fx.profile_dir(&a).exists(), "nothing was created");
}

#[test]
fn offline_the_launch_goes_on_with_the_stored_credential_and_a_warning() {
    // §12.3 step 1: a plain `Transient` continues with the stored credential.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.expire_access(&a);
    fx.http.push(
        Method::Post,
        &Fx::endpoints().token,
        Err(HttpError::PreSend("dns lookup failed".into())),
    );

    let launched = first_launch(&fx, &a, "a@x.co");

    assert_eq!(
        launched.warnings,
        [
            "could not refresh a@x.co first (pre-send); Claude Code will refresh it when it is online"
        ]
    );
    assert_eq!(
        fx.held_refresh_token(&launched.profile).as_deref(),
        Some("rt-a")
    );
}

#[test]
fn a_rotation_left_by_a_killed_session_is_captured_at_the_next_launch() {
    // Review Focus 1's end (§12.5 "Lazy capture"): quiescent again, so the next launch adopts it.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    fx.rotate_profile(&profile, "rt-a-2");

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();

    assert!(
        !launched.bootstrapped,
        "captured, the profile is the vault's generation"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert_eq!(seed(&profile).seed_fp, vault_fp(&fx, &a));
}

#[test]
fn a_vault_that_moved_on_re_bootstraps_the_profile() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    fx.put_vault(&a, &credential("a@x.co", "rt-a-2"));
    fx.script_valid(&profile, "a@x.co");

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();

    assert!(launched.bootstrapped);
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
    assert!(
        fx.displaced().is_empty(),
        "an older generation of this account is not saved"
    );
}

#[test]
fn a_stale_marked_profile_is_displaced_and_re_bootstrapped() {
    // §12.5's table: the replacement wins.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    fx.rotate_profile(&profile, "rt-a-session");
    fx.land_replacement(&a, "rt-a-new");
    fx.script_valid(&profile, "a@x.co");

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();

    assert!(launched.bootstrapped);
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a-new"),
        "never captured"
    );
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-new"));
    let displaced: Value = serde_json::from_slice(&fx.displaced()[0]).unwrap();
    assert_eq!(displaced["claudeAiOauth"]["refreshToken"], "rt-a-session");
}

#[test]
fn a_conflict_refuses_the_launch_and_overwrites_nothing() {
    // §12.5: both moved in an unknown order.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    fx.rotate_profile(&profile, "rt-a-2");
    fx.put_vault(&a, &credential("a@x.co", "rt-a-3"));

    let err = refused(
        fx.engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")),
    );

    assert_eq!(err.kind(), "profile-conflict", "{err}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-3"));
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
    assert_eq!(live_reservations(&profile), 0);
}

#[test]
fn an_unreadable_profile_credential_aborts_the_launch() {
    // §12.5 step 3: tagteam never overwrites a credential it could not read.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    fx.cc_writes_profile(&profile, |_| {});
    let (svc, acct) = fx.profile_item(&profile);
    fx.kc.set_unreadable(&svc, &acct, true);

    let err = refused(
        fx.engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")),
    );

    fx.kc.set_unreadable(&svc, &acct, false);
    assert_eq!(err.kind(), "unreadable", "{err}");
    assert_eq!(live_reservations(&profile), 0);
    assert_eq!(
        fx.held_refresh_token(&profile).as_deref(),
        Some("rt-a"),
        "untouched"
    );
}

#[test]
fn a_pending_rescue_is_settled_before_the_profile_is_compared() {
    // §6.2: provenance against the consumed generation would call the profile in step.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    fx.script_valid(&profile, "a@x.co");

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();

    assert!(
        launched.bootstrapped,
        "the vault moved on to the rescued successor"
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
}

#[test]
fn a_left_over_baseline_is_merged_back_before_the_seed() {
    // §12.4: a killed session's changes reach `~/.claude.json` at the next launch.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    session_adds_project(&profile, "/work/new");

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();

    assert!(!launched.bootstrapped);
    let merged = config_key(&fx.paths().global_config, "projects");
    assert_eq!(merged["/work/new"], json!({"allowedTools": ["Bash"]}));
    assert_eq!(
        config_key(&profile.join(".claude.json"), "projects"),
        merged,
        "then seeded from the merged default"
    );
}

#[test]
fn a_session_config_changed_on_both_sides_is_summarised_once_in_the_launch_s_warnings() {
    // §12.4 step 3: the default home's value wins, and one summary line says so.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    let allow = |config: &Path, tools: Value| {
        let mut projects = config_key(config, "projects");
        projects["/work/app"]["allowedTools"] = tools;
        splice_config_key(config, "projects", &projects);
    };
    allow(&profile.join(".claude.json"), json!(["Bash"]));
    allow(&fx.paths().global_config, json!(["Read"]));

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();

    assert_eq!(
        launched.warnings,
        [
            "1 key of position 1's session config changed on both sides while it ran; the default home's values were kept"
        ]
    );
    assert_eq!(
        config_key(&fx.paths().global_config, "projects")["/work/app"]["allowedTools"],
        json!(["Read"]),
        "the default home's value is kept"
    );
}

#[test]
fn a_merge_back_the_seed_or_the_bootstrap_runs_is_summarised_once_in_the_launch_s_warnings() {
    // §12.4 step 3. With no marker beside the baseline, the launch leaves the waiting merge-back
    // to the seed, or to the bootstrap when one is due; its summary is the launch's all the same.
    for bootstrap in [false, true] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let profile = killed_session(&fx, &a, "a@x.co");
        let allow = |config: &Path, tools: Value| {
            let mut projects = config_key(config, "projects");
            projects["/work/app"]["allowedTools"] = tools;
            splice_config_key(config, "projects", &projects);
        };
        allow(&profile.join(".claude.json"), json!(["Bash"]));
        allow(&fx.paths().global_config, json!(["Read"]));
        fs::remove_file(profile.join(MARKER_FILE)).unwrap();
        if bootstrap {
            fs::remove_file(profile.join(SEED_FILE)).unwrap();
            fx.script_valid(&profile, "a@x.co");
        }

        let launched = fx
            .engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
            .unwrap();

        assert_eq!(launched.bootstrapped, bootstrap);
        assert_eq!(
            launched.warnings,
            [
                "1 key of position 1's session config changed on both sides while it ran; the default home's values were kept"
            ],
            "bootstrap {bootstrap}"
        );
        assert_eq!(
            config_key(&fx.paths().global_config, "projects")["/work/app"]["allowedTools"],
            json!(["Read"]),
            "bootstrap {bootstrap}: the default home's value is kept"
        );
        drop(launched);
    }
}

#[test]
fn a_pending_merge_back_that_fails_aborts_with_the_profile_and_its_baseline_untouched() {
    // §12.4, B.44: seeding over them would discard the session's unmerged changes.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let cwd = fx.work_dir("app");
    let profile = killed_session(&fx, &a, "a@x.co");
    session_adds_project(&profile, "/work/new");
    let config = fs::read(profile.join(".claude.json")).unwrap();
    let baseline = fs::read(profile.join(".tagteam-baseline.json")).unwrap();

    block_home(&fx);
    let result = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd);
    unblock_home(&fx);

    let err = refused(result);
    assert_eq!(
        fs::read(profile.join(".claude.json")).unwrap(),
        config,
        "{err}"
    );
    assert_eq!(
        fs::read(profile.join(".tagteam-baseline.json")).unwrap(),
        baseline
    );
    assert_eq!(live_reservations(&profile), 0);
    assert_eq!(
        config_key(&fx.paths().global_config, "projects").get("/work/new"),
        None
    );

    // Once the default home can be written, the next launch merges it first.
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    assert!(!launched.bootstrapped);
    assert!(
        config_key(&fx.paths().global_config, "projects")
            .get("/work/new")
            .is_some()
    );
}

#[test]
fn a_moved_profile_s_unmerged_changes_are_merged_back_before_its_bootstrap_seeds_it() {
    // Decision 22, §12.4, B.44: the data directory moved, so the marker's spelling names a path
    // that is gone. The killed session's baseline is where the profile is now, and the
    // merge-back must find it there before the spelling-change bootstrap seeds over it.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    session_adds_project(&profile, "/work/new");
    let current = fx.spelling_for(&profile);
    let old = fx.dir.path().join("moved-from/sessions").join(a.as_str());
    let marker = match ProfileMarker::read(&profile) {
        Read::Present(m) => m,
        other => panic!("{other:?}"),
    };
    ProfileMarker {
        config_dir: old.to_str().unwrap().to_owned(),
        ..marker
    }
    .write(&profile)
    .unwrap();
    fx.script_valid(&profile, "a@x.co");

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();

    assert!(launched.bootstrapped, "§12.3: the spelling changed");
    assert_eq!(launched.spelling, current, "and the new one is recorded");
    let merged = config_key(&fx.paths().global_config, "projects");
    assert_eq!(
        merged["/work/new"],
        json!({"allowedTools": ["Bash"]}),
        "merged back first, from the baseline where the profile is"
    );
    assert_eq!(
        config_key(&profile.join(".claude.json"), "projects"),
        merged,
        "then bootstrapped and seeded from the merged default"
    );
    assert_eq!(
        config_key(&profile.join(".tagteam-baseline.json"), "projects"),
        merged,
        "the seed's new baseline"
    );
}

#[test]
fn a_launch_into_a_running_profile_joins_it_as_it_is() {
    // §12.5 step 3, B.28: no seed and no bootstrap under a running session, even when the vault
    // has moved on.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    let other = fx.hold_reservation(&profile);
    session_adds_project(&profile, "/work/new");
    let config = fs::read(profile.join(".claude.json")).unwrap();
    let seeded = seed(&profile);
    fx.put_vault(&a, &credential("a@x.co", "rt-a-2"));

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();

    assert!(!launched.bootstrapped);
    assert_eq!(
        fs::read(profile.join(".claude.json")).unwrap(),
        config,
        "not seeded"
    );
    assert_eq!(seed(&profile), seeded);
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a"));
    assert_eq!(fx.spawner.specs().len(), 1);
    assert_eq!(live_reservations(&profile), 2);
    drop(other);
}

#[test]
fn dead_reservations_are_removed_and_the_profile_counts_as_quiescent() {
    // §12.5 step 2.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    let dead = {
        let gone = fx.hold_reservation(&profile);
        gone.path().to_path_buf()
    };
    assert!(dead.exists());

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();

    assert!(!dead.exists());
    assert_eq!(live_reservations(&profile), 1);
    assert_eq!(launched.reservation.path().parent(), dead.parent());
}

#[test]
fn while_a_launch_holds_its_reservation_the_account_is_session_owned() {
    // §12.5: `remove`, `switch` and the gate all see the reservation once the locks are released.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let launched = first_launch(&fx, &a, "a@x.co");

    assert_eq!(fx.engine.remove(&a).unwrap_err().kind(), "session-owned");
    assert_eq!(
        fx.switch_to(&a, false).expect_err("refused").kind(),
        "session-owned"
    );
    let vault = fx.vault_bytes(&a).unwrap();
    assert!(matches!(
        fx.engine
            .refresh_stored(fx.cc.as_ref(), &a, &vault)
            .unwrap(),
        GateOutcome::Owned(OwnedBy::Session)
    ));
    drop(launched);
}

#[test]
fn a_signal_before_the_locks_launches_nothing() {
    // §12.5 "Signals": every lock wait before the spawn is a cancellation point. So is the
    // moment before the gate refresh (R10.2, §14.1): a signal that has landed stops the launch
    // before it spends the vault's refresh token.
    for due in [false, true] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let cwd = fx.work_dir("app");
        if due {
            fx.expire_access(&a);
        }
        fx.env.cancel.request(libc::SIGINT);

        let err = refused(fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd));

        let _ = fx.env.cancel.take();
        assert_eq!(err.kind(), "interrupted", "due {due}: {err}");
        assert_eq!(err.signal(), Some(libc::SIGINT), "due {due}");
        assert!(!fx.profile_dir(&a).exists(), "due {due}");
        assert_eq!(token_requests(&fx), 0, "due {due}: nothing was sent");
    }
}

#[test]
fn a_live_reservation_already_under_this_pid_refuses_the_launch_and_is_left_alone() {
    // Task 4: an orphaned `claude` of an earlier process with this pid still holds
    // `<pid>.lock`. Replacing it would hide a running session.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    let mine = profile
        .join(LAUNCH_DIR)
        .join(format!("{}.lock", std::process::id()));
    let orphan = FlockGuard::try_lock(&mine).unwrap().unwrap();
    let before = fs::read(&mine).unwrap();

    let err = refused(
        fx.engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")),
    );

    assert_eq!(err.kind(), "launch-unreachable", "{err}");
    assert!(
        err.to_string().contains(&mine.display().to_string()),
        "{err}"
    );
    assert!(
        err.to_string()
            .starts_with("the profile could not be reserved for this launch"),
        "it says what happened, never that the launch command failed: {err}"
    );
    assert_eq!(fs::read(&mine).unwrap(), before, "never replaced");
    assert!(mutation_lock_free(&fx.env));
    drop(orphan);
}

#[test]
fn each_scrubbed_variable_this_process_has_set_is_named_once_in_the_launch_s_warnings() {
    // §12.5 "Environment", Decision 18: the CLI records which are set in `Env.vars`.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let mut env = fx.env.clone();
    for name in ["ANTHROPIC_API_KEY", "USE_STAGING_OAUTH"] {
        env.vars.insert(name.into(), OsString::new());
    }
    fx.script_valid(&fx.profile_dir(&a), "a@x.co");

    let launched = fx
        .engine_with_env(env)
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();

    let w = &launched.warnings;
    assert_eq!(w.len(), 2, "{w:?}");
    assert!(w[0].starts_with("ANTHROPIC_API_KEY is set here"), "{w:?}");
    assert!(w[1].starts_with("USE_STAGING_OAUTH is set here"), "{w:?}");
    assert!(
        launched
            .env
            .remove
            .contains(&OsString::from("ANTHROPIC_API_KEY")),
        "and scrubbed: {:?}",
        launched.env
    );
}

#[test]
fn a_quarantined_target_due_for_a_refresh_is_refused_and_never_refreshed() {
    // §7.2, §7.4: refused as `Dead` is, before anything is created.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
    fx.expire_access(&a);

    let err = refused(
        fx.engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")),
    );

    assert_eq!(err.kind(), "relogin-required", "{err}");
    assert_eq!(token_requests(&fx), 0, "never refreshed");
    assert!(!fx.profile_dir(&a).exists(), "nothing was created");
}

#[test]
fn a_quarantined_target_launches_once_with_the_warning_that_it_needs_a_new_login() {
    // §7.2: usable only while its access token lasts. Controller ruling (Task 8): `plan_run`
    // never reads the quarantine, so the row read under the locks decides, and a quarantine set
    // since planning is warned about too, once.
    for planned_first in [false, true] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned_before = row(&fx, &a);
        fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
        let planned = if planned_first {
            planned_before
        } else {
            row(&fx, &a)
        };
        fx.script_valid(&fx.profile_dir(&a), "a@x.co");

        let launched = fx
            .engine
            .launch(&planned, claude_bin(), &fx.work_dir("app"))
            .unwrap();

        assert_eq!(
            launched.warnings,
            [
                "a@x.co (position 1) needs a new login: its stored refresh token can no longer be used; it works only until its current access token expires"
            ],
            "planned before the quarantine: {planned_first}"
        );
        assert_eq!(token_requests(&fx), 0);
    }
}

#[test]
fn a_join_launches_whatever_the_vault_holds_and_touches_no_credential() {
    // Controller ruling (Task 10): §12.5 step 3 joins a running session without seeding, and
    // its sync only creates missing links. The running session's Claude Code owns the token, so
    // a join reads, refreshes and writes no stored credential: a vault entry gone missing, or a
    // quarantine (§7.4), even one whose access token is due, refuses only a quiescent launch.
    for case in ["vault entry missing", "quarantined", "quarantined, due"] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let profile = killed_session(&fx, &a, "a@x.co");
        let other = fx.hold_reservation(&profile);
        let linked = links(&profile);
        fs::remove_file(profile.join("skills")).unwrap();
        if case == "vault entry missing" {
            fx.kc.delete(SERVICE, a.as_str()).unwrap();
        } else {
            if case == "quarantined, due" {
                fx.expire_access(&a);
            }
            fx.quarantine(&a, "invalid_grant", &vault_fp(&fx, &a));
        }
        let read = |name: &str| fs::read(profile.join(name)).unwrap();
        let files = [
            ".credentials.json",
            ".claude.json",
            ".tagteam-baseline.json",
            ".tagteam-seed.json",
            ".tagteam-profile.json",
            ".tagteam-links.json",
        ]
        .map(|name| (name, read(name)));
        let items = fx.kc.items();

        let launched = fx
            .engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
            .unwrap_or_else(|e| panic!("{case}: {e}"));

        assert!(!launched.bootstrapped, "{case}");
        assert!(
            launched.warnings.is_empty(),
            "{case}: {:?}",
            launched.warnings
        );
        assert_eq!(fx.kc.items(), items, "{case}: no Keychain item touched");
        for (name, bytes) in &files {
            assert_eq!(&read(name), bytes, "{case}: {name} untouched");
        }
        assert_eq!(token_requests(&fx), 0, "{case}: nothing refreshed");
        assert_eq!(fx.spawner.specs().len(), 1, "{case}: nothing validated");
        assert_eq!(
            links(&profile),
            linked,
            "{case}: only the missing link made"
        );
        assert_eq!(live_reservations(&profile), 2, "{case}");
        drop(other);
    }
}

#[test]
fn a_profile_path_linked_elsewhere_refuses_the_launch_before_anything_is_touched_there() {
    // Controller ruling (Task 9): the real-directory check comes first under the locks. Through
    // a link at the profile path, step 2 would remove the dead reservations at its target and
    // merge back the baseline there before `mark_profile` refused.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    session_adds_project(&profile, "/work/new");
    let target = fx.dir.path().join("elsewhere").join(a.as_str());
    fs::create_dir(target.parent().unwrap()).unwrap();
    fs::rename(&profile, &target).unwrap();
    symlink(&target, &profile).unwrap();
    let before = dir_tree(&target);
    let global = fs::read(fx.paths().global_config).unwrap();

    let err = refused(
        fx.engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")),
    );

    assert_eq!(err.kind(), "invalid-input", "{err}");
    assert!(
        err.to_string().contains(&profile.display().to_string()),
        "{err}"
    );
    assert_eq!(
        dir_tree(&target),
        before,
        "nothing touched at the link's target"
    );
    assert_eq!(
        fs::read(fx.paths().global_config).unwrap(),
        global,
        "nothing merged back from it"
    );
    assert!(mutation_lock_free(&fx.env));
}

#[test]
fn a_join_into_a_profile_marked_for_another_account_refuses_before_its_links_are_synced() {
    // A marker naming another account refuses first (as for a quiescent launch), so a join
    // never makes links in a profile that is not the account's.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    let other = fx.hold_reservation(&profile);
    let skills = profile.join("skills");
    fs::remove_file(&skills).unwrap();
    let marker = match ProfileMarker::read(&profile) {
        Read::Present(m) => m,
        other => panic!("{other:?}"),
    };
    ProfileMarker {
        account_id: AccountId::from_string("0192-someone-else"),
        ..marker
    }
    .write(&profile)
    .unwrap();

    let err = refused(
        fx.engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")),
    );

    assert_eq!(err.kind(), "invalid-input", "{err}");
    assert!(err.to_string().contains("names another account"), "{err}");
    assert!(
        fs::symlink_metadata(&skills).is_err(),
        "no link was made in it"
    );
    assert_eq!(live_reservations(&profile), 1, "only the running session's");
    drop(other);
}

#[test]
fn a_reservation_directory_that_cannot_be_used_refuses_the_launch_naming_it() {
    // Controller ruling (Task 4): the reservation calls report I/O errors without a path, so the
    // refusal names the directory. A file in its place is no live reservation of this pid.
    for case in ["unlistable", "a file"] {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let profile = killed_session(&fx, &a, "a@x.co");
        let dir = profile.join(LAUNCH_DIR);
        if case == "unlistable" {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
        } else {
            fs::remove_dir_all(&dir).unwrap();
            fs::write(&dir, "").unwrap();
        }

        let result = fx
            .engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"));

        if case == "unlistable" {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let err = refused(result);
        assert_eq!(err.kind(), "io", "{case}: {err}");
        assert!(
            err.to_string().contains(&dir.display().to_string()),
            "{case}: {err}"
        );
        assert!(mutation_lock_free(&fx.env), "{case}");
    }
}

#[test]
fn an_unreadable_run_shell_refuses_the_launch_before_anything_else() {
    // §12.8, as `plan_run` (R8.1): under a marker that cannot be read the outer home is unknown,
    // and the engine's `env` is still the run shell's. A launch would take that profile for the
    // default home: its login for the live one, and its entries for the ones to share.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.add("c@x.co", "rt-c");
    let shell = fx.make_profile(&a);
    fs::write(shell.join(MARKER_FILE), "{ torn").unwrap();
    let engine = fx.engine_located(fx.shell_env(&shell));

    let err = refused(engine.launch(&row(&fx, &b), claude_bin(), &fx.work_dir("app")));

    assert_eq!(err.kind(), "run-shell-unreadable", "{err}");
    assert!(!fx.profile_dir(&b).exists(), "nothing was created");
    assert!(fx.spawner.specs().is_empty());
}

#[test]
fn a_real_copy_of_a_must_share_entry_refuses_a_quiescent_launch_as_a_split() {
    // R10.4, §12.2: the sync's `ProfileSplit` is the launch's refusal, worded by its cause.
    // Memory kept in the profile's own `projects/` is never linked over or seeded around.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    let copy = profile.join("projects");
    fs::remove_file(&copy).unwrap();
    fs::create_dir(&copy).unwrap();
    fs::write(copy.join("notes.md"), "mine\n").unwrap();

    let err = refused(
        fx.engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")),
    );

    assert_eq!(err.kind(), "profile-split", "{err}");
    let text = err.to_string();
    assert!(
        text.contains(&format!("{} is a real copy", copy.display())),
        "{text}"
    );
    assert!(text.contains("merge the two by hand"), "{text}");
    assert_eq!(
        fs::read_to_string(copy.join("notes.md")).unwrap(),
        "mine\n",
        "the copy is untouched"
    );
    assert_eq!(live_reservations(&profile), 0);
    assert_eq!(fx.spawner.specs().len(), 1, "never validated again");
}

#[test]
fn a_join_over_a_must_share_link_gone_stale_refuses_until_the_session_ends() {
    // R10.4, §12.2: the outer home's `projects/` moved behind a link while a session runs. The
    // profile's link still points at the old place, and a join never changes a running
    // session's links, so the two would use different memory.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    let other = fx.hold_reservation(&profile);
    let source = fx.env.home.join(".claude/projects");
    let moved = fx.env.home.join("dotfiles/projects");
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::rename(&source, &moved).unwrap();
    symlink(&moved, &source).unwrap();
    let link = profile.join("projects");
    let before = fs::read_link(&link).unwrap();

    let err = refused(
        fx.engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app")),
    );

    assert_eq!(err.kind(), "profile-split", "{err}");
    let text = err.to_string();
    assert!(text.contains(&link.display().to_string()), "{text}");
    assert!(text.contains("end that session"), "{text}");
    assert_eq!(
        fs::read_link(&link).unwrap(),
        before,
        "the running session's link"
    );
    assert_eq!(live_reservations(&profile), 1, "only the running session's");
    drop(other);
}

#[test]
fn the_login_check_is_skipped_by_a_launch_that_bootstrapped() {
    // §12.3: its validation already ran, under the locks.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let launched = first_launch(&fx, &a, "a@x.co");

    fx.engine
        .check_login(&launched, claude_bin(), &fx.work_dir("app"))
        .unwrap();

    assert_eq!(fx.spawner.specs().len(), 1);
    assert!(
        fx.engine
            .finish_run(launched, LaunchEnd::Exited(0))
            .is_empty()
    );
}

#[test]
fn every_other_launch_is_checked_in_its_own_directory_with_its_reservation_held() {
    // §12.3 "Every launch is checked", §12.5 step 6: after the locks, before the spawn.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    let cwd = fx.work_dir("app/src");
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    fx.script_valid(&profile, "a@x.co");

    fx.engine
        .check_login(&launched, claude_bin(), &cwd)
        .unwrap();

    let specs = fx.spawner.specs();
    assert_eq!(specs.len(), 2);
    assert_eq!(
        specs[1].cwd.as_deref(),
        Some(cwd.as_path()),
        "where claude runs, so a project's own settings count"
    );
    assert_eq!(
        specs[1].program,
        claude_bin(),
        "the binary the session will run"
    );
    assert!(mutation_lock_free(&fx.env));
    assert_eq!(live_reservations(&profile), 1);
    assert!(
        fx.engine
            .finish_run(launched, LaunchEnd::Exited(0))
            .is_empty()
    );
}

#[test]
fn every_refusing_outcome_of_the_login_check_keeps_the_profile() {
    // §12.3's table outside a bootstrap, B.62.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    let spelling = fx.spelling_for(&profile);
    let cwd = fx.work_dir("app");
    let rows = [
        (
            auth_reply(0, auth_helper(&spelling).to_string().as_bytes()),
            "login-overridden",
        ),
        (
            auth_reply(
                0,
                auth_status("/somewhere/else", "a@x.co")
                    .to_string()
                    .as_bytes(),
            ),
            "login-drifted",
        ),
        (Captured::TimedOut, "login-unknown"),
        (auth_reply(0, b"not json"), "login-unknown"),
        (
            Captured::SpawnFailed("No such file or directory".into()),
            "launch-unreachable",
        ),
    ];
    for (reply, kind) in rows {
        let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
        fx.spawner.push(reply);

        let err = fx
            .engine
            .check_login(&launched, claude_bin(), &cwd)
            .unwrap_err();

        assert_eq!(err.kind(), kind, "{err}");
        let notices = fx.engine.finish_run(launched, LaunchEnd::Refused);
        assert!(notices.is_empty(), "{kind}: {notices:?}");
        assert!(profile.join(MARKER_FILE).exists(), "{kind}: kept");
        assert!(!seed(&profile).needs_bootstrap, "{kind}");
    }
}

#[test]
fn an_invalid_login_check_keeps_the_profile_and_the_next_launch_bootstraps_it() {
    // §12.3, B.62: only a bootstrap's validation deletes a profile.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    let cwd = fx.work_dir("app");
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    fx.script_auth(1, &auth_logged_out(&fx.spelling_for(&profile)));

    let err = fx
        .engine
        .check_login(&launched, claude_bin(), &cwd)
        .unwrap_err();

    assert_eq!(err.kind(), "login-invalid", "{err}");
    assert!(
        seed(&profile).needs_bootstrap,
        "recorded for the next launch"
    );
    assert!(
        fx.engine
            .finish_run(launched, LaunchEnd::Refused)
            .is_empty()
    );
    assert!(profile.join(MARKER_FILE).exists(), "kept");

    fx.script_valid(&profile, "a@x.co");
    let next = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    assert!(next.bootstrapped);
    assert!(!seed(&profile).needs_bootstrap);
    assert!(fx.engine.finish_run(next, LaunchEnd::Exited(0)).is_empty());
}

#[test]
fn a_signal_during_the_login_check_interrupts_the_launch() {
    // §12.5 "Signals": the wait for the check is a cancellation point, and its process killed.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    let cwd = fx.work_dir("app");
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    fx.env.cancel.request(libc::SIGTERM);
    fx.spawner.push(Captured::Interrupted(libc::SIGTERM));

    let err = fx
        .engine
        .check_login(&launched, claude_bin(), &cwd)
        .unwrap_err();

    let _ = fx.env.cancel.take();
    assert_eq!(err.kind(), "interrupted", "{err}");
    assert_eq!(err.signal(), Some(libc::SIGTERM));
    assert!(!seed(&profile).needs_bootstrap);
    assert!(
        fx.engine
            .finish_run(launched, LaunchEnd::Refused)
            .is_empty()
    );
}

#[test]
fn a_launch_refused_by_its_login_check_runs_its_exit_handling_at_once() {
    // §12.3, §12.5 step 6: as if `claude` had exited at once.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    let cwd = fx.work_dir("app");
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    let own = launched.reservation.path().to_path_buf();
    assert!(profile.join(".tagteam-baseline.json").exists(), "seeded");
    fx.script_auth(0, &auth_helper(&fx.spelling_for(&profile)));
    assert_eq!(
        fx.engine
            .check_login(&launched, claude_bin(), &cwd)
            .unwrap_err()
            .kind(),
        "login-overridden"
    );

    let notices = fx.engine.finish_run(launched, LaunchEnd::Refused);

    assert!(notices.is_empty(), "{notices:?}");
    assert!(!own.exists(), "unlinked");
    assert!(
        !profile.join(".tagteam-baseline.json").exists(),
        "merged back, with nothing to merge"
    );
    assert!(mutation_lock_free(&fx.env));
}

#[test]
fn the_last_session_out_captures_a_rotation_and_merges_its_config_back() {
    // §12.5 "When the child exits".
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let launched = first_launch(&fx, &a, "a@x.co");
    let profile = launched.profile.clone();
    let own = launched.reservation.path().to_path_buf();
    session_adds_project(&profile, "/work/new");
    fx.rotate_profile(&profile, "rt-a-2");

    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert_eq!(
        seed(&profile).seed_fp,
        vault_fp(&fx, &a),
        "the seed moves with the capture"
    );
    assert_eq!(
        config_key(&fx.paths().global_config, "projects")["/work/new"],
        json!({"allowedTools": ["Bash"]}),
        "merged back (§12.4)"
    );
    assert!(!profile.join(".tagteam-baseline.json").exists());
    assert!(!own.exists(), "unlinked last");
    assert!(mutation_lock_free(&fx.env));
}

#[test]
fn of_two_sessions_the_last_one_out_captures_a_rotation_either_made() {
    // Review Focus 3: the second joins without a seed or a bootstrap; the last one out captures.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    // Another `tagteam run` of the account starts first, in another process.
    let other = fx.hold_reservation(&profile);
    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
        .unwrap();
    assert!(!launched.bootstrapped);
    assert!(
        !profile.join(".tagteam-baseline.json").exists(),
        "joined without a seed"
    );
    fx.rotate_profile(&profile, "rt-a-2");
    // The other exits first. Its exit handling finds this session running, so it only unlinks.
    let theirs = other.path().to_path_buf();
    drop(other);
    fs::remove_file(theirs).unwrap();

    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert_eq!(seed(&profile).seed_fp, vault_fp(&fx, &a));
}

#[test]
fn the_first_session_out_leaves_capture_and_merge_back_to_the_last() {
    // Review Focus 3, the other order.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let cwd = fx.work_dir("app");
    let launched = first_launch(&fx, &a, "a@x.co");
    let profile = launched.profile.clone();
    let own = launched.reservation.path().to_path_buf();
    // A second session joined, in another process.
    let other = fx.hold_reservation(&profile);
    session_adds_project(&profile, "/work/new");
    fx.rotate_profile(&profile, "rt-a-2");

    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

    assert!(notices.is_empty(), "{notices:?}");
    assert!(!own.exists(), "only the unlink");
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a"),
        "nothing is captured under a running session"
    );
    assert!(
        profile.join(".tagteam-baseline.json").exists(),
        "the merge-back waits"
    );
    assert_eq!(
        config_key(&fx.paths().global_config, "projects").get("/work/new"),
        None
    );

    // The other session ends with its `tagteam` killed: the next launch completes both.
    drop(other);
    let next = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    assert!(!next.bootstrapped);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert!(
        config_key(&fx.paths().global_config, "projects")
            .get("/work/new")
            .is_some()
    );
    drop(next);
}

#[test]
fn a_background_session_keeps_the_profile_until_it_exits() {
    // §12.5, §12.6: a `daemon` record counts, and capture and merge-back wait for it.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let cwd = fx.work_dir("app");
    let launched = first_launch(&fx, &a, "a@x.co");
    let profile = launched.profile.clone();
    let daemon = fx.live_record(&profile, 4242, "daemon");
    session_adds_project(&profile, "/work/new");
    fx.rotate_profile(&profile, "rt-a-2");

    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

    assert!(notices.is_empty(), "{notices:?}");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert!(profile.join(".tagteam-baseline.json").exists());
    assert_eq!(
        live_reservations(&profile),
        0,
        "its own reservation is gone all the same"
    );

    // The daemon shuts down gracefully and removes its record (Appendix A.7).
    fs::remove_file(daemon).unwrap();
    let next = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    assert!(!next.bootstrapped);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
    assert!(
        config_key(&fx.paths().global_config, "projects")
            .get("/work/new")
            .is_some()
    );
    drop(next);
}

#[test]
fn a_failed_merge_back_keeps_the_baseline_and_the_next_launch_merges_it() {
    // §12.4: the profile's changes and the baseline are kept, so it is retried before any re-seed.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let cwd = fx.work_dir("app");
    let launched = first_launch(&fx, &a, "a@x.co");
    let profile = launched.profile.clone();
    let own = launched.reservation.path().to_path_buf();
    session_adds_project(&profile, "/work/new");

    block_home(&fx);
    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));
    unblock_home(&fx);

    assert_eq!(notices.len(), 1, "one notice: {notices:?}");
    assert!(notices[0].contains("did not finish"), "{}", notices[0]);
    assert!(profile.join(".tagteam-baseline.json").exists());
    assert!(!own.exists(), "the unlink still runs, last");

    let next = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    assert_eq!(
        config_key(&fx.paths().global_config, "projects")["/work/new"],
        json!({"allowedTools": ["Bash"]})
    );
    drop(next);
}

#[test]
fn a_merge_back_keeps_the_default_where_both_changed_and_says_so_once() {
    // Review Focus 4 (§12.4): one key changed on both sides, a new project, a removed MCP
    // server. Every other byte of the default file stays as it was.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let launched = first_launch(&fx, &a, "a@x.co");
    let mine = launched.profile.join(".claude.json");
    let theirs = fx.paths().global_config;
    let mut projects = config_key(&mine, "projects");
    projects["/work/app"]["allowedTools"] = json!(["Bash"]);
    projects["/work/new"] = json!({"allowedTools": []});
    splice_config_key(&mine, "projects", &projects);
    splice_config_key(&mine, "mcpServers", &json!({}));
    let mut defaults = config_key(&theirs, "projects");
    defaults["/work/app"]["allowedTools"] = json!(["Read"]);
    splice_config_key(&theirs, "projects", &defaults);
    let rest = |doc: &[u8]| {
        remove_top_level(&remove_top_level(doc, "projects").unwrap(), "mcpServers").unwrap()
    };
    let before = rest(&fs::read(&theirs).unwrap());

    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

    assert_eq!(
        notices,
        [
            "1 key of position 1's session config changed on both sides while it ran; the default home's values were kept"
        ]
    );
    let merged = config_key(&theirs, "projects");
    assert_eq!(
        merged["/work/app"]["allowedTools"],
        json!(["Read"]),
        "the default wins"
    );
    assert_eq!(merged["/work/app"]["history"], json!(["x"]), "untouched");
    assert_eq!(merged["/work/new"], json!({"allowedTools": []}), "applied");
    assert_eq!(
        config_key(&theirs, "mcpServers"),
        json!({}),
        "the removal applied"
    );
    assert_eq!(
        rest(&fs::read(&theirs).unwrap()),
        before,
        "every other byte as it was"
    );
}

#[test]
fn an_api_key_helper_added_to_the_shared_settings_refuses_the_next_launch_and_keeps_the_profile() {
    // Review Focus 5, second half (§12.3 "Every launch is checked"). The helper is in the
    // shared `settings.json`, which the profile reads through its link; the scripted check
    // answers as Claude Code then does.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = finished_session(&fx, &a, "a@x.co");
    let settings = fx.env.home.join(".claude/settings.json");
    fs::write(&settings, r#"{"theme":"dark","apiKeyHelper":"~/bin/key"}"#).unwrap();
    assert_eq!(
        fs::read_link(profile.join("settings.json")).unwrap(),
        fs::canonicalize(&settings).unwrap()
    );
    let cwd = fx.work_dir("app");
    let launched = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
    fx.script_auth(0, &auth_helper(&fx.spelling_for(&profile)));

    let err = fx
        .engine
        .check_login(&launched, claude_bin(), &cwd)
        .unwrap_err();

    assert!(
        matches!(&err, EngineError::LoginOverridden { method, .. } if method == "api_key_helper"),
        "{err}"
    );
    assert!(!seed(&profile).needs_bootstrap);
    assert!(
        fx.engine
            .finish_run(launched, LaunchEnd::Refused)
            .is_empty()
    );
    assert!(profile.join(MARKER_FILE).exists(), "kept");
}

#[test]
fn a_conflict_at_exit_is_one_notice_ending_with_its_remedy_and_the_merge_back_and_unlink_still_run()
{
    // §12.5 "When the child exits": the profile rotated and the vault moved while the session
    // ran, so nothing is captured. The next launch refuses as well, so the notice ends with what
    // resolves the conflict, never with "nothing is lost". No log line names the account's
    // label (§14.2, B.69): the conflict's message does.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let launched = first_launch(&fx, &a, "a@x.co");
    let profile = launched.profile.clone();
    let own = launched.reservation.path().to_path_buf();
    let label = launched.account.label.clone();
    session_adds_project(&profile, "/work/new");
    fx.rotate_profile(&profile, "rt-a-2");
    fx.put_vault(&a, &credential("a@x.co", "rt-a-3"));

    let (notices, logs) = capture_logs(|| fx.engine.finish_run(launched, LaunchEnd::Exited(0)));

    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].starts_with("exit handling for position 1 did not finish")
            && notices[0].ends_with("log in again with `tagteam add` to resolve it"),
        "{}",
        notices[0]
    );
    assert!(!notices[0].contains("nothing is lost"), "{}", notices[0]);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-3"));
    assert_eq!(fx.held_refresh_token(&profile).as_deref(), Some("rt-a-2"));
    assert_eq!(
        config_key(&fx.paths().global_config, "projects")["/work/new"],
        json!({"allowedTools": ["Bash"]}),
        "the merge-back still ran"
    );
    assert!(!profile.join(".tagteam-baseline.json").exists());
    assert!(!own.exists(), "unlinked");
    assert!(
        logs.iter()
            .any(|l| l.contains("WARN") && l.contains("profile-conflict")),
        "logged by its kind: {logs:?}"
    );
    assert!(
        logs.iter()
            .all(|l| !l.contains(label.as_str()) && !l.contains("a@x.co")),
        "{logs:?}"
    );
}

#[test]
fn a_merge_back_that_fails_after_the_capture_failed_is_logged_by_its_kind() {
    // §14: a contained error is logged, never discarded. The capture's failure is the one
    // reported; the merge-back's is logged.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let launched = first_launch(&fx, &a, "a@x.co");
    let profile = launched.profile.clone();
    let own = launched.reservation.path().to_path_buf();
    session_adds_project(&profile, "/work/new");
    fx.rotate_profile(&profile, "rt-a-2");
    fx.put_vault(&a, &credential("a@x.co", "rt-a-3"));

    block_home(&fx);
    let (notices, logs) = capture_logs(|| fx.engine.finish_run(launched, LaunchEnd::Exited(0)));
    unblock_home(&fx);

    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].ends_with("log in again with `tagteam add` to resolve it"),
        "{}",
        notices[0]
    );
    let merge_back = logs
        .iter()
        .filter(|l| l.contains("WARN") && l.contains("could not be merged back"))
        .collect::<Vec<_>>();
    assert_eq!(merge_back.len(), 1, "{logs:?}");
    assert!(merge_back[0].contains("kind="), "{}", merge_back[0]);
    assert!(
        merge_back[0].contains(&format!("account={a}")),
        "{}",
        merge_back[0]
    );
    assert!(profile.join(".tagteam-baseline.json").exists(), "kept");
    assert!(!own.exists(), "unlinked");
}

#[test]
fn a_malformed_record_at_exit_leaves_capture_and_merge_back_waiting_and_its_name_out_of_the_log() {
    // §12.6: a record that cannot be read counts as a session, so the last one out is not this
    // one. The record's file name is not tagteam's to choose, so the log gives the state only
    // (§14.2, B.69).
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let launched = first_launch(&fx, &a, "a@x.co");
    let profile = launched.profile.clone();
    let own = launched.reservation.path().to_path_buf();
    session_adds_project(&profile, "/work/new");
    fx.plant_record(&profile, "alice@example.com", b"[1,");

    let (notices, logs) = capture_logs(|| fx.engine.finish_run(launched, LaunchEnd::Exited(0)));

    assert!(notices.is_empty(), "{notices:?}");
    assert!(
        profile.join(".tagteam-baseline.json").exists(),
        "the merge-back waits"
    );
    assert!(!own.exists(), "unlinked");
    assert!(
        logs.iter()
            .any(|l| l.contains("WARN") && l.contains("state=\"unreadable\"")),
        "{logs:?}"
    );
    assert!(
        logs.iter().all(|l| !l.contains("alice@example.com")),
        "{logs:?}"
    );
}

#[test]
fn a_dead_reservation_is_removed_without_its_name_in_the_log() {
    // §12.5 step 2. Only tagteam names its own reservations `<pid>.lock`; a file of any other
    // name there is removed all the same once it is dead, and its name is never logged.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let profile = killed_session(&fx, &a, "a@x.co");
    let stray = profile.join(LAUNCH_DIR).join("alice@example.com.lock");
    fs::write(&stray, b"").unwrap();

    let (launched, logs) = capture_logs(|| {
        fx.engine
            .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"))
            .unwrap()
    });

    assert!(!stray.exists(), "removed");
    assert!(
        logs.iter()
            .any(|l| l.contains("dead launch reservation") && l.contains("removed=2")),
        "{logs:?}"
    );
    assert!(
        logs.iter().all(|l| !l.contains("alice@example.com")),
        "{logs:?}"
    );
    drop(launched);
}

#[test]
fn a_rotation_in_a_profile_that_names_no_identity_is_not_captured_at_exit() {
    // R11.1, M4a's Decision 9 at exit: a rotation no identity says is the account's is neither
    // captured nor ignored. It is the one notice; the merge-back and the unlink still run.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let launched = first_launch(&fx, &a, "a@x.co");
    let profile = launched.profile.clone();
    let own = launched.reservation.path().to_path_buf();
    let config = profile.join(".claude.json");
    let without = remove_top_level(&fs::read(&config).unwrap(), "oauthAccount").unwrap();
    fs::write(&config, without).unwrap();
    session_adds_project(&profile, "/work/new");
    fx.rotate_profile(&profile, "rt-a-2");

    let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].ends_with("so it cannot be told to be the account's"),
        "the next launch stops on it too, so the notice ends with it: {}",
        notices[0]
    );
    assert!(notices[0].contains("names no identity"), "{}", notices[0]);
    assert_eq!(
        fx.vault_refresh_token(&a).as_deref(),
        Some("rt-a"),
        "not captured"
    );
    assert_eq!(
        config_key(&fx.paths().global_config, "projects")["/work/new"],
        json!({"allowedTools": ["Bash"]}),
        "a failed capture does not stop the merge-back"
    );
    assert!(!own.exists(), "unlinked");
}

#[cfg(feature = "test-hooks")]
mod hooked {
    use std::sync::{Arc, Mutex};

    use common::{block_rescue, rescue_files, unblock_rescue};
    use tagteam_provider::Clock;

    use super::*;

    #[test]
    fn a_target_removed_before_the_locks_is_not_launched() {
        // Decision 7, B.47: decided again under the locks.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned = row(&fx, &a);
        let cwd = fx.work_dir("app");
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let id = a.clone();
        fx.engine.on_point(
            "launch-before-locks",
            Box::new(move || {
                other.remove(&id).unwrap();
            }),
        );

        let err = refused(fx.engine.launch(&planned, claude_bin(), &cwd));

        assert_eq!(err.kind(), "target-changed", "{err}");
        assert!(err.to_string().contains("was removed"), "{err}");
        assert!(!fx.profile_dir(&a).exists(), "nothing was created");
    }

    #[test]
    fn a_target_removed_before_the_launch_reads_its_vault_is_not_launched() {
        // Decision 14, B.47: a `remove` that completes after `plan_run`, before the launch's
        // first read of the account, is the same race the re-check under the locks answers,
        // not a missing credential (`invalid-input`).
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned = row(&fx, &a);
        let cwd = fx.work_dir("app");
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let id = a.clone();
        fx.engine.on_point(
            "launch-before-freshen",
            Box::new(move || {
                other.remove(&id).unwrap();
            }),
        );

        let err = refused(fx.engine.launch(&planned, claude_bin(), &cwd));

        assert_eq!(err.kind(), "target-changed", "{err}");
        assert!(err.to_string().contains("was removed"), "{err}");
        assert!(!fx.profile_dir(&a).exists(), "nothing was created");
        assert_eq!(token_requests(&fx), 0, "nothing was refreshed");
    }

    #[test]
    fn a_target_that_became_the_live_login_before_the_locks_is_left_to_plain_claude() {
        // §12.5 step 1: otherwise one token would get a default copy and a profile copy.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned = row(&fx, &a);
        let cwd = fx.work_dir("app");
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let req = fx.switch_request(&a, false);
        fx.engine.on_point(
            "launch-before-locks",
            Box::new(move || {
                other.switch(req.clone()).unwrap();
            }),
        );

        let err = refused(fx.engine.launch(&planned, claude_bin(), &cwd));

        assert_eq!(err.kind(), "target-changed", "{err}");
        assert!(err.to_string().contains("became the live login"), "{err}");
        assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
        assert!(!fx.profile_dir(&a).exists());
    }

    #[test]
    fn a_target_whose_login_became_an_api_key_is_refused_under_the_locks() {
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned = row(&fx, &a);
        let cwd = fx.work_dir("app");
        let (db, id) = (fx.env.data_dir().join("tagteam.db"), a.to_string());
        fx.engine.on_point(
            "launch-before-locks",
            Box::new(move || {
                rusqlite::Connection::open(&db)
                    .unwrap()
                    .execute("UPDATE accounts SET kind = 'api_key' WHERE id = ?1", [&id])
                    .unwrap();
            }),
        );

        let err = refused(fx.engine.launch(&planned, claude_bin(), &cwd));

        assert_eq!(err.kind(), "api-key-account", "{err}");
    }

    #[test]
    fn a_target_whose_stored_credential_went_away_before_the_locks_is_refused_under_them() {
        // Controller ruling (Task 8): `plan_run` never reads the vault, and the read before the
        // locks is behind us, so the locks read it again before anything is created.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned = row(&fx, &a);
        let cwd = fx.work_dir("app");
        let (kc, id) = (fx.kc.clone(), a.to_string());
        fx.engine.on_point(
            "launch-before-locks",
            Box::new(move || {
                kc.delete(SERVICE, &id).unwrap();
            }),
        );

        let err = refused(fx.engine.launch(&planned, claude_bin(), &cwd));

        assert_eq!(err.kind(), "invalid-input", "{err}");
        assert!(err.to_string().contains("no stored credential"), "{err}");
        assert!(!fx.profile_dir(&a).exists(), "nothing was created");
    }

    #[test]
    fn a_target_quarantined_before_the_locks_whose_token_is_due_is_refused_under_them() {
        // §9.4 step 1's rule for a switch, applied to a launch: a refresh that finished while
        // this launch waited may have quarantined the target (§7.4). §7.2 then refuses it once
        // its access token is due.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned = row(&fx, &a);
        let cwd = fx.work_dir("app");
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let (kc, id, fp) = (fx.kc.clone(), a.clone(), vault_fp(&fx, &a));
        let mut expiring: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
        expiring["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms() + 60_000);
        fx.engine.on_point(
            "launch-before-locks",
            Box::new(move || {
                let store = other.store().unwrap();
                store.set_quarantine(&id, "successor_lost", &fp, 1).unwrap();
                kc.put(SERVICE, id.as_str(), expiring.to_string().as_bytes());
            }),
        );

        let err = refused(fx.engine.launch(&planned, claude_bin(), &cwd));

        assert_eq!(err.kind(), "relogin-required", "{err}");
        assert_eq!(token_requests(&fx), 0, "never refreshed");
        assert!(!fx.profile_dir(&a).exists(), "nothing was created");
    }

    #[test]
    fn a_quarantine_landing_before_the_freshen_refuses_only_a_quiescent_launch() {
        // Fix round 1: the gate answers a quarantine that still binds `Dead` before it asks who
        // owns the account (§7.3), so the freshen goes on and the locks decide. A join uses no
        // stored credential (§12.5 step 3); a quiescent launch is refused as `Dead` is (§7.2).
        for joining in [true, false] {
            let fx = Fx::new();
            let a = two_accounts(&fx);
            let running = joining.then(|| {
                let profile = killed_session(&fx, &a, "a@x.co");
                let other = fx.hold_reservation(&profile);
                (profile, other)
            });
            let planned = row(&fx, &a);
            let cwd = fx.work_dir("app");
            let other = Arc::new(fx.engine_with_env(fx.env.clone()));
            let (kc, id, fp) = (fx.kc.clone(), a.clone(), vault_fp(&fx, &a));
            let mut expiring: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
            expiring["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms() + 60_000);
            fx.engine.on_point(
                "launch-before-freshen",
                Box::new(move || {
                    let store = other.store().unwrap();
                    store.set_quarantine(&id, "invalid_grant", &fp, 1).unwrap();
                    kc.put(SERVICE, id.as_str(), expiring.to_string().as_bytes());
                }),
            );
            let global = fs::read(fx.paths().global_config).unwrap();

            let result = fx.engine.launch(&planned, claude_bin(), &cwd);

            assert_eq!(
                token_requests(&fx),
                0,
                "joining {joining}: nothing was sent"
            );
            match running {
                Some((profile, other)) => {
                    let launched = result.unwrap_or_else(|e| panic!("joining: {e}"));
                    assert!(!launched.bootstrapped);
                    assert!(launched.warnings.is_empty(), "{:?}", launched.warnings);
                    assert_eq!(live_reservations(&profile), 2);
                    drop(other);
                }
                None => {
                    let err = refused(result);
                    assert_eq!(err.kind(), "relogin-required", "{err}");
                    assert!(!fx.profile_dir(&a).exists(), "nothing was created");
                    assert_eq!(fs::read(fx.paths().global_config).unwrap(), global);
                    assert!(fx.spawner.specs().is_empty(), "nothing was validated");
                }
            }
        }
    }

    #[test]
    fn a_gate_refusal_before_the_locks_refuses_a_quiescent_launch_before_anything_is_written() {
        // Fix round 2, §12.3 step 1 and §7.2: the gate sends, adopts and compares only for an
        // account no session owns (§7.3 step 2), but a session can start between its answer
        // and this launch's locks. Its refusals are carried across them: a quiescent launch
        // gets exactly the error and advice, before anything is written, and a join, which
        // touches no credential (§12.5 step 3), goes on. The race session is a reservation
        // taken at `launch-before-locks`.
        let cases = [
            (
                "rescued",
                "a@x.co (position 1) has a refreshed token that is not in the vault yet: the refresh succeeded, but the vault could not be written; the new token is in rescue/; retry once the vault can be written",
            ),
            (
                "unpersisted",
                "a@x.co (position 1) needs a new login: its stored refresh token can no longer be used; log in with `claude`, then run `tagteam add`",
            ),
            (
                "rescue unreadable",
                "a@x.co (position 1) has a refreshed token that is not in the vault yet: a pending rescue could not be adopted; retry once the vault can be written",
            ),
            (
                "conflict",
                "position 1 (a@x.co)'s session profile and the vault both moved since they last agreed; log in again with `tagteam add` to resolve it",
            ),
        ];
        for (case, refusal) in cases {
            for joining in [false, true] {
                let at = format!("{case}, joining {joining}");
                let fx = Fx::new();
                let a = two_accounts(&fx);
                let profile = killed_session(&fx, &a, "a@x.co");
                match case {
                    "rescue unreadable" => {
                        let dir = fx.env.data_dir().join("rescue");
                        fs::create_dir_all(&dir).unwrap();
                        fs::write(dir.join(format!("{a}-0-000000000000.json")), "{ torn").unwrap();
                    }
                    "conflict" => {
                        fx.rotate_profile(&profile, "rt-a-2");
                        fx.put_vault(&a, &credential("a@x.co", "rt-a-3"));
                    }
                    _ => fx.script_refresh(Some("rt-a-2")),
                }
                fx.expire_access(&a);
                if matches!(case, "rescued" | "unpersisted") {
                    fx.kc.set_fail_write(SERVICE, true);
                }
                if case == "unpersisted" {
                    block_rescue(&fx);
                }
                // Once the gate has answered: the vault can be written again, so a rescue could
                // be adopted now, and the best-effort `successor_lost` quarantine is as if it
                // were never recorded. Neither may let a quiescent launch through.
                let other = Arc::new(fx.engine_with_env(fx.env.clone()));
                let (kc, id) = (fx.kc.clone(), a.clone());
                let race = profile
                    .join(LAUNCH_DIR)
                    .join(format!("{}-race.lock", std::process::id()));
                let session = Arc::new(Mutex::new(None));
                let started = session.clone();
                fx.engine.on_point(
                    "launch-before-locks",
                    Box::new(move || {
                        kc.set_fail_write(SERVICE, false);
                        other.store().unwrap().clear_quarantine(&id).unwrap();
                        if joining {
                            *started.lock().unwrap() = FlockGuard::try_lock(&race).unwrap();
                        }
                    }),
                );
                let before = dir_tree(&profile);
                let global = fs::read(fx.paths().global_config).unwrap();

                let result = fx
                    .engine
                    .launch(&row(&fx, &a), claude_bin(), &fx.work_dir("app"));

                unblock_rescue(&fx);
                if joining {
                    let launched = result.unwrap_or_else(|e| panic!("{at}: {e}"));
                    assert!(!launched.bootstrapped, "{at}");
                    assert_eq!(live_reservations(&profile), 2, "{at}");
                    drop(launched);
                } else {
                    let err = refused(result);
                    assert_eq!(err.to_string(), refusal, "{at}");
                    assert_eq!(dir_tree(&profile), before, "{at}: nothing written");
                    assert_eq!(
                        fs::read(fx.paths().global_config).unwrap(),
                        global,
                        "{at}: nothing merged back"
                    );
                }
                if case == "rescued" {
                    assert_eq!(
                        rescue_files(&fx),
                        1,
                        "{at}: the gate's rescue stays where it is"
                    );
                    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"), "{at}");
                }
                assert_eq!(fx.spawner.specs().len(), 1, "{at}: nothing was validated");
                drop(session);
            }
        }
    }

    #[test]
    fn the_gate_finds_the_account_busy_while_the_launch_holds_its_lock() {
        // §7.3 step 1: a reservation can only appear while the gate cannot hold the lock.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let planned = row(&fx, &a);
        let cwd = fx.work_dir("app");
        fx.script_valid(&fx.profile_dir(&a), "a@x.co");
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let seen = Arc::new(Mutex::new(None));
        let (cc, id, vault, record) = (
            fx.cc.clone(),
            a.clone(),
            fx.vault_bytes(&a).unwrap(),
            seen.clone(),
        );
        fx.engine.on_point(
            "launch-locked",
            Box::new(move || {
                let outcome = other.refresh_stored(cc.as_ref(), &id, &vault);
                *record.lock().unwrap() = Some(matches!(outcome, Ok(GateOutcome::Busy)));
            }),
        );

        let launched = fx.engine.launch(&planned, claude_bin(), &cwd).unwrap();

        assert_eq!(*seen.lock().unwrap(), Some(true));
        drop(launched);
    }

    #[test]
    fn a_signal_during_exit_handling_defers_it_and_loses_nothing() {
        // §12.5 "After `claude` exits", B.63: its lock wait is a cancellation point; lazy
        // capture and the next launch complete what it left.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let cwd = fx.work_dir("app");
        let launched = first_launch(&fx, &a, "a@x.co");
        let profile = launched.profile.clone();
        let own = launched.reservation.path().to_path_buf();
        session_adds_project(&profile, "/work/new");
        fx.rotate_profile(&profile, "rt-a-2");
        let cancel = fx.env.cancel.clone();
        fx.engine.on_point(
            "exit-before-locks",
            Box::new(move || cancel.request(libc::SIGTERM)),
        );

        let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

        let _ = fx.env.cancel.take();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
        assert!(profile.join(".tagteam-baseline.json").exists());
        assert_eq!(
            probe_lock(&own).unwrap(),
            LockProbe::Free,
            "left in place; its lock went with this process's hold"
        );

        let next = fx.engine.launch(&row(&fx, &a), claude_bin(), &cwd).unwrap();
        assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a-2"));
        assert!(
            config_key(&fx.paths().global_config, "projects")
                .get("/work/new")
                .is_some()
        );
        assert_eq!(live_reservations(&profile), 1);
        drop(next);
    }

    #[test]
    fn a_failure_once_the_exit_locks_are_held_still_unlinks_the_reservation_last() {
        // R11.2: the reservation is created and removed only under the locks (§12.5), and once
        // they are held its unlink runs whatever failed before it, with one notice.
        let fx = Fx::new();
        let a = two_accounts(&fx);
        let launched = first_launch(&fx, &a, "a@x.co");
        let profile = launched.profile.clone();
        let own = launched.reservation.path().to_path_buf();
        session_adds_project(&profile, "/work/new");
        fx.engine.fail_at(Some("exit-locked"));

        let notices = fx.engine.finish_run(launched, LaunchEnd::Exited(0));

        fx.engine.fail_at(None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("injected failure at exit-locked"),
            "the first cause: {}",
            notices[0]
        );
        assert!(!own.exists(), "unlinked all the same");
        assert!(
            profile.join(".tagteam-baseline.json").exists(),
            "nothing after the failure ran"
        );
        assert!(mutation_lock_free(&fx.env));
        assert!(AccountLock::try_acquire(&fx.env, &a).unwrap().is_some());
    }
}
