//! §12.1: what `tagteam run` launches, decided before any lock: a session for an account, or
//! plain `claude` with the environment it was given, which inside a run shell is the outer
//! home's (§12.8).

mod common;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use common::{API_KEY, Fx, two_accounts};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::run::{RunPlan, RunRequest};
use tagteam_provider::process::SpawnSpec;
use tagteam_provider::profile::MARKER_FILE;

/// `tagteam run [ACCOUNT] -- --resume` from `cwd`, for the default provider.
fn request(account: Option<&AccountId>, cwd: &Path) -> RunRequest {
    RunRequest {
        account: account.cloned(),
        provider: None,
        require_session: false,
        cwd: cwd.to_path_buf(),
        args: vec![OsString::from("--resume")],
    }
}

/// `req` with `--require-session`.
fn requiring(req: RunRequest) -> RunRequest {
    RunRequest {
        require_session: true,
        ..req
    }
}

/// `tagteam map <id> <dir>` (§12.7), stored under the directory's canonical path.
fn map(fx: &Fx, dir: &Path, id: &AccountId) {
    let path = fs::canonicalize(dir).unwrap();
    fx.engine
        .store()
        .unwrap()
        .set_mapping(path.to_str().unwrap(), &fx.provider(), id, 1)
        .unwrap();
}

/// What Decision 7's race leaves: a mapping of `dir` whose account is gone, as `plan_run` reads
/// it between the two reads. The store's cascade never leaves one, so it is written directly.
fn dangling_mapping(fx: &Fx, dir: &Path) {
    let path = fs::canonicalize(dir).unwrap();
    let db = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    // The bundled SQLite is built with SQLITE_DEFAULT_FOREIGN_KEYS=1, so every new connection
    // enforces `mappings.account_id`'s reference and would refuse the orphan. Off on this raw
    // connection only; the store's own connections keep enforcing it.
    db.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    db.execute(
        "INSERT INTO mappings (path, provider, account_id, added_at) VALUES (?1, ?2, 'gone', 1)",
        [path.to_str().unwrap(), fx.provider().as_str()],
    )
    .unwrap();
}

fn plain(plan: RunPlan) -> (SpawnSpec, Option<String>) {
    match plan {
        RunPlan::Plain { spec, warning } => (spec, warning),
        RunPlan::Session { account, .. } => {
            panic!(
                "a session for position {}, not plain claude",
                account.position
            )
        }
    }
}

fn session(plan: RunPlan) -> (AccountId, ProviderId, PathBuf) {
    match plan {
        RunPlan::Session {
            account,
            provider,
            launch,
        } => (account.id, provider, launch),
        RunPlan::Plain { spec, .. } => panic!("plain claude, not a session: {spec:?}"),
    }
}

#[test]
fn with_no_account_and_no_mapping_plain_claude_runs_with_an_untouched_environment() {
    let fx = Fx::new();
    two_accounts(&fx);
    let bin = fx.fake_bin("claude");
    let cwd = fx.work_dir("app");

    let (spec, warning) = plain(
        fx.engine_on_path(&bin)
            .plan_run(&request(None, &cwd))
            .unwrap(),
    );

    assert_eq!(spec.program, bin.join("claude"));
    assert_eq!(spec.args, [OsString::from("--resume")]);
    assert!(spec.set.is_empty() && spec.remove.is_empty(), "{spec:?}");
    assert_eq!(spec.cwd, None, "exec keeps the directory it was given");
    assert_eq!(warning, None);
}

#[test]
fn with_no_store_plain_claude_runs_and_nothing_is_created() {
    // §5: a decision that only reads creates nothing.
    let fx = Fx::new();
    let bin = fx.fake_bin("claude");
    let cwd = fx.work_dir("app");

    plain(
        fx.engine_on_path(&bin)
            .plan_run(&request(None, &cwd))
            .unwrap(),
    );

    assert!(!fx.env.data_dir().exists());
}

#[test]
fn the_nearest_mapped_ancestor_decides() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let bin = fx.fake_bin("claude");
    let deeper = fx.work_dir("app/src/lib");
    let sibling = fx.work_dir("other");
    map(&fx, &fx.env.home.join("work"), &b);
    map(&fx, &fx.env.home.join("work/app"), &a);
    let engine = fx.engine_on_path(&bin);

    let (id, provider, launch) = session(engine.plan_run(&request(None, &deeper)).unwrap());
    assert_eq!(
        (id, provider),
        (a, fx.provider()),
        "`~/work/app` is nearer than `~/work`"
    );
    assert_eq!(launch, bin.join("claude"));

    // `~/work/other` inherits `~/work`'s mapping: b, the live login, so plain claude.
    let (_, warning) = plain(engine.plan_run(&request(None, &sibling)).unwrap());
    assert_eq!(warning, None);
}

#[test]
fn a_mapping_whose_account_went_away_runs_plain_claude_with_a_warning() {
    // §12.1 and Decision 7.
    let fx = Fx::new();
    two_accounts(&fx);
    let cwd = fx.work_dir("app");
    dangling_mapping(&fx, &cwd);

    let (spec, warning) = plain(
        fx.engine_on_path(&fx.fake_bin("claude"))
            .plan_run(&request(None, &cwd))
            .unwrap(),
    );

    let warning = warning.expect("a warning names the mapping");
    assert!(warning.contains("was removed"), "{warning}");
    assert!(spec.set.is_empty() && spec.remove.is_empty(), "{spec:?}");
}

#[test]
fn require_session_refuses_wherever_plain_claude_would_run() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let engine = fx.engine_on_path(&fx.fake_bin("claude"));
    let unmapped = fx.work_dir("unmapped");
    let orphaned = fx.work_dir("orphaned");
    dangling_mapping(&fx, &orphaned);

    for (req, why) in [
        (request(None, &unmapped), "no mapping applies"),
        (request(None, &orphaned), "was removed"),
        (request(Some(&b), &unmapped), "is the live login"),
    ] {
        let err = engine.plan_run(&requiring(req)).unwrap_err();
        assert_eq!(err.kind(), "requires-session", "{err}");
        assert!(err.to_string().contains(why), "{err}");
    }
    // Where a session would start, the flag changes nothing.
    session(
        engine
            .plan_run(&requiring(request(Some(&a), &unmapped)))
            .unwrap(),
    );
}

#[test]
fn an_api_key_account_is_refused_named_or_mapped() {
    let fx = Fx::new();
    two_accounts(&fx);
    let k = fx.add_api_key(API_KEY);
    let position = fx
        .engine
        .store()
        .unwrap()
        .account(&k)
        .unwrap()
        .unwrap()
        .position;
    let cwd = fx.work_dir("app");
    map(&fx, &cwd, &k);
    let engine = fx.engine_on_path(&fx.fake_bin("claude"));

    for req in [request(Some(&k), &cwd), request(None, &cwd)] {
        let err = engine.plan_run(&req).unwrap_err();
        assert!(
            matches!(err, EngineError::ApiKeyAccount { position: p } if p == position),
            "{err}"
        );
        assert_eq!(err.kind(), "api-key-account");
    }
}

#[test]
fn the_live_login_runs_plain_claude_and_any_other_account_gets_a_session() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let bin = fx.fake_bin("claude");
    let engine = fx.engine_on_path(&bin);
    let cwd = fx.work_dir("app");

    let (_, warning) = plain(engine.plan_run(&request(Some(&b), &cwd)).unwrap());
    assert_eq!(
        warning, None,
        "§12.1: never two copies of one rotating token"
    );

    let planned = session(engine.plan_run(&request(Some(&a), &cwd)).unwrap());
    assert_eq!(planned, (a, fx.provider(), bin.join("claude")));
}

#[test]
fn a_missing_launch_command_fails_before_the_mapping_and_the_live_login_and_changes_nothing() {
    let fx = Fx::new();
    let empty = fx.dir.path().join("empty-bin");
    fs::create_dir_all(&empty).unwrap();
    // A `claude` that is not executable is not a launch command either.
    fs::write(empty.join("claude"), "#!/bin/sh\n").unwrap();
    let cwd = fx.work_dir("app");

    let err = fx
        .engine_on_path(&empty)
        .plan_run(&request(None, &cwd))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::LaunchCommandMissing { command } if command == "claude"),
        "{err}"
    );
    assert_eq!(err.kind(), "launch-command-missing");
    assert!(!fx.env.data_dir().exists(), "nothing was created");

    // It comes before the live-login check and `--require-session`'s refusal.
    let a = fx.add("a@x.co", "rt-a");
    let err = fx
        .engine_on_path(&empty)
        .plan_run(&requiring(request(Some(&a), &cwd)))
        .unwrap_err();
    assert_eq!(err.kind(), "launch-command-missing", "{err}");
}

#[test]
fn an_account_that_does_not_exist_is_refused() {
    let fx = Fx::new();
    two_accounts(&fx);
    let nope = AccountId::from_string("nope");

    let err = fx
        .engine_on_path(&fx.fake_bin("claude"))
        .plan_run(&request(Some(&nope), &fx.work_dir("app")))
        .unwrap_err();

    assert_eq!(err.kind(), "no-such-account", "{err}");
}

#[test]
fn a_named_account_of_another_provider_than_the_one_asked_for_is_refused() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let req = RunRequest {
        provider: Some(ProviderId::new("fake-agent")),
        ..request(Some(&a), &fx.work_dir("app"))
    };

    let err = fx
        .engine_on_path(&fx.fake_bin("claude"))
        .plan_run(&req)
        .unwrap_err();

    assert_eq!(err.kind(), "invalid-input", "{err}");
}

#[test]
fn an_unreadable_live_login_refuses_rather_than_guess() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let engine = fx.engine_on_path(&fx.fake_bin("claude"));
    let cwd = fx.work_dir("app");
    fs::write(fx.paths().global_config, "{ torn").unwrap();

    let err = engine.plan_run(&request(Some(&a), &cwd)).unwrap_err();

    assert_eq!(err.kind(), "unreadable", "{err}");
}

#[test]
fn inside_a_run_shell_plain_claude_gets_the_outer_home_back_and_nothing_is_scrubbed() {
    // §12.1, §12.8: as it would have run outside the session.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let bin = fx.fake_bin("claude");
    let profile = fx.make_profile(&a);
    let process = Fx::with_path(&fx.shell_env(&profile), &bin);
    let engine = fx.engine_located(process.clone());

    let (spec, _) = plain(
        engine
            .plan_run(&request(Some(&b), &fx.work_dir("app")))
            .unwrap(),
    );

    assert!(spec.set.is_empty(), "{spec:?}");
    assert_eq!(
        spec.remove,
        [
            OsString::from("CLAUDE_CONFIG_DIR"),
            OsString::from("CLAUDE_SECURESTORAGE_CONFIG_DIR")
        ],
        "both were undefined in the outer home"
    );
    assert!(!spec.remove.contains(&OsString::from("ANTHROPIC_API_KEY")));
    // M4a's `apply_outer_home` clones the `Env` it is given, and `Env::clone` shares the
    // `Cancel` (M3a): the outer `Env` keeps the process's token, which `run` forwards signals
    // through (Decision 1).
    process.cancel.request(libc::SIGTERM);
    assert_eq!(
        engine.cancel().requested(),
        Some(libc::SIGTERM),
        "the outer Env's cancel token is the process's"
    );
}

#[test]
fn a_custom_outer_home_is_restored_as_the_marker_records_it() {
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let bin = fx.fake_bin("claude");
    let custom = fx.dir.path().join("custom-home");
    let mut outer = fx.env.clone();
    outer.claude_config_dir = Some(custom.clone().into_os_string());
    let profile = fx.profile_dir(&a);
    fx.write_marker(&profile, &a, &outer);
    let engine = fx.engine_located(Fx::with_path(&fx.shell_env(&profile), &bin));

    let (spec, _) = plain(
        engine
            .plan_run(&request(None, &fx.work_dir("app")))
            .unwrap(),
    );

    assert_eq!(
        spec.set,
        [(OsString::from("CLAUDE_CONFIG_DIR"), custom.into_os_string())]
    );
    assert_eq!(
        spec.remove,
        [OsString::from("CLAUDE_SECURESTORAGE_CONFIG_DIR")]
    );
}

#[test]
fn an_empty_outer_config_dir_is_unset_never_exported() {
    // Appendix A.1: tagteam treats an empty CLAUDE_CONFIG_DIR as unset and never exports one.
    // A defined-but-empty CLAUDE_SECURESTORAGE_CONFIG_DIR means `~/.claude`, so it stays.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    let bin = fx.fake_bin("claude");
    let mut outer = fx.env.clone();
    outer.claude_config_dir = Some(OsString::new());
    outer.claude_securestorage_config_dir = Some(OsString::new());
    let profile = fx.profile_dir(&a);
    fx.write_marker(&profile, &a, &outer);
    let engine = fx.engine_located(Fx::with_path(&fx.shell_env(&profile), &bin));

    let (spec, _) = plain(
        engine
            .plan_run(&request(None, &fx.work_dir("app")))
            .unwrap(),
    );

    assert_eq!(
        spec.set,
        [(
            OsString::from("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
            OsString::new()
        )]
    );
    assert_eq!(spec.remove, [OsString::from("CLAUDE_CONFIG_DIR")]);
}

#[test]
fn inside_a_run_shell_another_account_gets_a_session() {
    // §12.1: a session can start a session of another account.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let c = fx.add("c@x.co", "rt-c");
    fx.add("b@x.co", "rt-b");
    let bin = fx.fake_bin("claude");
    let profile = fx.make_profile(&a);
    let engine = fx.engine_located(Fx::with_path(&fx.shell_env(&profile), &bin));

    let (id, _, launch) = session(
        engine
            .plan_run(&request(Some(&c), &fx.work_dir("app")))
            .unwrap(),
    );

    assert_eq!((id, launch), (c, bin.join("claude")));
}

#[test]
fn a_directory_reached_through_a_symlink_finds_the_mapping_of_its_canonical_path() {
    // §12.7: mappings are canonical paths, so `plan_run` looks `cwd` up as `map` stores it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let bin = fx.fake_bin("claude");
    let real = fx.work_dir("app");
    map(&fx, &real, &a);
    let link = fx.env.home.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let engine = fx.engine_on_path(&bin);

    for cwd in [
        link.clone(),
        link.join("."),
        fx.env.home.join("work/../link"),
    ] {
        let (id, _, _) = session(engine.plan_run(&request(None, &cwd)).unwrap());
        assert_eq!(id, a, "{}", cwd.display());
    }
    // A subdirectory reached through the link inherits it too.
    let sub = real.join("src");
    fs::create_dir_all(&sub).unwrap();
    let (id, _, _) = session(engine.plan_run(&request(None, &link.join("src"))).unwrap());
    assert_eq!(id, a);
}

#[test]
fn an_unreadable_run_shell_refuses_every_plan_before_anything_else() {
    // §12.8: under a marker that cannot be read the outer home is unknown. A named account
    // would otherwise be compared against the profile's login, taken for the live one.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let profile = fx.make_profile(&a);
    let marker = profile.join(MARKER_FILE);
    fs::write(&marker, "{ torn").unwrap();
    let missing = fx.dir.path().join("no-bin");
    let engine = fx.engine_located(Fx::with_path(&fx.shell_env(&profile), &missing));

    // Even with no launch command on PATH: the shell's refusal comes first.
    for req in [
        request(Some(&b), &fx.work_dir("app")),
        request(None, &fx.work_dir("app")),
    ] {
        let err = engine.plan_run(&req).unwrap_err();
        assert_eq!(err.kind(), "run-shell-unreadable", "{err}");
    }
}
