//! §12.5 "Launch": a session's start under `MutationGuard` and the account lock, what it
//! decides again under them, and its reservation. Task 11 appends the per-launch login check
//! and the exit handling.

mod common;

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use common::{
    Fx, claude_bin, credential, mutation_lock_free, splice_config_key, token_requests,
    two_accounts, vault_fp,
};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::EngineError;
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::launch::Launched;
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::store::AccountRow;
use tagteam_engine::vault::SERVICE;
use tagteam_provider::Read;
use tagteam_provider::flock::{FlockGuard, LockProbe, probe_lock};
use tagteam_provider::http::{HttpError, Method};
use tagteam_provider::profile::{LAUNCH_DIR, ProfileMarker, Seed, launch_reservations};
use tagteam_provider::splice::get_top_level;

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
fn a_refresh_whose_successor_reached_only_rescue_aborts_the_launch_before_any_lock() {
    // §12.3 step 1: the vault's generation is consumed.
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

#[cfg(feature = "test-hooks")]
mod hooked {
    use std::sync::{Arc, Mutex};

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
}
