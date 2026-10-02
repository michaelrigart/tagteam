//! §12.8 inside a run shell: detection by the profile marker alone, the outer home the engine
//! runs on, and the refusals (Review Focus 1 and 4).

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use common::{API_KEY, FakeFx, Fx, credential, sent_refresh_tokens, token_requests};
use serde_json::json;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::active::{ActiveOutcome, ActiveTrigger};
use tagteam_engine::refresh::{GateOutcome, OwnedBy};
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::session::detect_run_shell;
use tagteam_engine::views::StatusView;
use tagteam_engine::{Engine, EngineError};
use tagteam_fake::FAKE_AGENT;
use tagteam_provider::profile::{MARKER_FILE, ProfileMarker, RunShell};
use tagteam_provider::{Clock, Provider};

fn registry(fx: &Fx) -> ProviderRegistry {
    ProviderRegistry::new().with(fx.cc.clone())
}

/// A call's error kind, or `None` when it went through.
fn kind<T>(r: Result<T, EngineError>) -> Option<&'static str> {
    r.err().map(|e| e.kind())
}

/// Every entry under `dir`, never following a link: a link's target, a file's bytes, or nothing
/// for a directory.
fn tree(dir: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let held = if meta.file_type().is_symlink() {
                Some(
                    fs::read_link(&path)
                        .unwrap()
                        .into_os_string()
                        .into_encoded_bytes(),
                )
            } else if meta.is_dir() {
                dirs.push(path.clone());
                None
            } else {
                Some(fs::read(&path).unwrap())
            };
            out.insert(path, held);
        }
    }
    out
}

/// Every engine call that changes accounts or the live login (§9.2, §12.8), with its error kind,
/// and the active-token refresh, which §12.8 lets run on the default home inside a run shell.
fn account_changes(
    fx: &Fx,
    engine: &Engine,
    a: &AccountId,
) -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("switch", kind(engine.switch(fx.switch_request(a, false)))),
        (
            "switch --force",
            kind(engine.switch(fx.switch_request(a, true))),
        ),
        ("add", kind(engine.add_live(fx.add_options()))),
        (
            "add-token",
            kind(engine.add_token(fx.add_token_options(API_KEY))),
        ),
        ("remove", kind(engine.remove(a))),
        ("alias", kind(engine.set_alias(a, Some("work")))),
        ("disable", kind(engine.set_disabled(a, true))),
        ("move", kind(engine.move_to(a, 2))),
        (
            "active refresh",
            kind(engine.refresh_active(&fx.provider(), ActiveTrigger::Expired)),
        ),
    ]
}

#[test]
fn a_marker_outside_sessions_is_a_run_shell() {
    // §15.2 "Run shell": the path's location is not consulted.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let elsewhere = fx.dir.path().join("elsewhere/profile");
    let marker = fx.write_marker(&elsewhere, &a, &fx.env);
    let (shell, effective) = detect_run_shell(&fx.shell_env(&elsewhere), &registry(&fx));
    assert_eq!(
        shell,
        RunShell::Inside {
            profile: elsewhere,
            marker
        }
    );
    assert_eq!(effective.claude_config_dir, None, "the outer home set none");
}

#[test]
fn a_profile_path_without_a_marker_is_not_a_run_shell() {
    // The path rule that `Env::inside_run_shell` applied is gone (L312).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.profile_dir(&a);
    fs::create_dir_all(&dir).unwrap();
    let env = fx.shell_env(&dir);
    let (shell, effective) = detect_run_shell(&env, &registry(&fx));
    assert_eq!(shell, RunShell::Outside);
    assert_eq!(
        effective.claude_config_dir, env.claude_config_dir,
        "unchanged"
    );
}

#[test]
fn detection_holds_with_xdg_data_home_changed_inside_the_shell() {
    // Review Focus 1: a tool that runs with its own `XDG_DATA_HOME` is still in the run shell.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let mut env = fx.shell_env(&profile);
    env.xdg_data_home = Some(fx.dir.path().join("tool-data"));
    let (shell, effective) = detect_run_shell(&env, &registry(&fx));
    assert!(matches!(shell, RunShell::Inside { .. }), "{shell:?}");
    assert_eq!(
        effective.xdg_data_home, env.xdg_data_home,
        "only the provider's own variables are restored"
    );
    let engine = fx.engine_located(env);
    assert!(matches!(
        engine.add_live(fx.add_options()),
        Err(EngineError::InsideRunShell)
    ));
}

#[test]
fn a_profile_reached_through_a_symlink_is_a_run_shell() {
    // Review Focus 1: the marker is read through the link, and the profile keeps the spelling
    // the variable gave it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let profile = fx.make_profile(&a);
    let link = fx.dir.path().join("via-link");
    symlink(&profile, &link).unwrap();
    let (shell, _) = detect_run_shell(&fx.shell_env(&link), &registry(&fx));
    let RunShell::Inside {
        profile: found,
        marker,
    } = shell
    else {
        panic!("{shell:?}")
    };
    assert_eq!(found, link);
    assert_eq!(marker.account_id, a);
}

#[test]
fn the_engine_runs_on_the_outer_home_the_marker_records() {
    // §12.8: `outer` comes back exactly, a defined-but-empty CLAUDE_SECURESTORAGE_CONFIG_DIR
    // included (Appendix A.1).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let mut outer = fx.env.clone();
    outer.claude_config_dir = Some(fx.env.home.join("custom-claude").into_os_string());
    outer.claude_securestorage_config_dir = Some("".into());
    let profile = fx.profile_dir(&a);
    fx.write_marker(&profile, &a, &outer);
    let (_, effective) = detect_run_shell(&fx.shell_env(&profile), &registry(&fx));
    assert_eq!(effective.claude_config_dir, outer.claude_config_dir);
    assert_eq!(effective.claude_securestorage_config_dir, Some("".into()));
    assert_eq!(effective.home, fx.env.home);
    assert_eq!(effective.data_dir(), fx.env.data_dir());
}

#[test]
fn an_unreadable_marker_leaves_the_environment_alone_and_names_the_file() {
    for bad in [
        &b"{ torn"[..],
        br#"{"format": "tagteam-profile", "version": 2}"#,
        b"[1]",
    ] {
        let fx = Fx::new();
        let dir = fx.dir.path().join("p");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(MARKER_FILE), bad).unwrap();
        let env = fx.shell_env(&dir);
        let (shell, effective) = detect_run_shell(&env, &registry(&fx));
        let RunShell::Unreadable { marker, .. } = shell else {
            panic!("{}: {shell:?}", String::from_utf8_lossy(bad))
        };
        assert_eq!(marker, dir.join(MARKER_FILE));
        assert_eq!(effective.claude_config_dir, env.claude_config_dir);
    }
}

#[test]
fn a_marker_for_another_provider_is_unreadable() {
    // Its `outer` is another provider's record, so Claude Code's outer home is unknown.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.dir.path().join("p");
    fs::create_dir_all(&dir).unwrap();
    ProfileMarker {
        provider: ProviderId::new(FAKE_AGENT),
        account_id: a,
        config_dir: dir.display().to_string(),
        outer: serde_json::json!({"FAKEAGENT_HOME": null}),
    }
    .write(&dir)
    .unwrap();
    let (shell, _) = detect_run_shell(&fx.shell_env(&dir), &registry(&fx));
    assert!(matches!(shell, RunShell::Unreadable { .. }), "{shell:?}");
}

#[test]
fn the_first_registered_provider_with_a_marker_decides() {
    // Decision 5: FakeAgent's own variable finds its profile; with Claude Code's variable set
    // too, Claude Code, registered first, decides.
    let ffx = FakeFx::new();
    let registry = ProviderRegistry::new()
        .with(ffx.fx.cc.clone())
        .with(ffx.fake.clone());
    let fake_profile = ffx.fx.dir.path().join("fake-profile");
    fs::create_dir_all(&fake_profile).unwrap();
    let fake_marker = ProfileMarker {
        provider: ProviderId::new(FAKE_AGENT),
        account_id: AccountId::from_string("fake-1"),
        config_dir: fake_profile.display().to_string(),
        outer: ffx.fake.outer_home(&ffx.fx.env),
    };
    fake_marker.write(&fake_profile).unwrap();
    let mut env = ffx.fx.env.clone();
    env.vars.insert(
        "FAKEAGENT_HOME".into(),
        fake_profile.clone().into_os_string(),
    );
    let (shell, effective) = detect_run_shell(&env, &registry);
    assert_eq!(
        shell,
        RunShell::Inside {
            profile: fake_profile,
            marker: fake_marker
        }
    );
    assert_eq!(
        effective.var("FAKEAGENT_HOME"),
        None,
        "the outer home set none"
    );

    let cc_profile = ffx.fx.dir.path().join("cc-profile");
    let cc_marker = ffx
        .fx
        .write_marker(&cc_profile, &AccountId::from_string("cc-1"), &ffx.fx.env);
    env.claude_config_dir = Some(cc_profile.clone().into_os_string());
    let (shell, _) = detect_run_shell(&env, &registry);
    assert_eq!(
        shell,
        RunShell::Inside {
            profile: cc_profile,
            marker: cc_marker
        }
    );
}

#[test]
fn inside_a_run_shell_the_gate_never_refreshes_the_default_live_login() {
    // Review Focus 1. The profile's `.claude.json` names the session's own account, a, so an
    // engine that took the profile for the default home would see b, the default home's live
    // login, as inactive, and refresh the very token Claude Code in ~/.claude is using.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let profile = fx.make_profile(&a);
    fx.expire_access(&b);
    fx.script_refresh(Some("rt-b2"));
    let snapshot = fx.vault_bytes(&b).unwrap();
    let located = fx.engine_located(fx.shell_env(&profile));
    let outcome = located
        .refresh_stored(fx.cc.as_ref(), &b, &snapshot)
        .unwrap();
    assert!(
        matches!(outcome, GateOutcome::Owned(OwnedBy::Live)),
        "{outcome:?}"
    );
    assert_eq!(token_requests(&fx), 0);
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b"));
    // The fixture has teeth: the same environment taken at face value refreshes it.
    let face_value = fx.engine_with_env(fx.shell_env(&profile));
    let outcome = face_value
        .refresh_stored(fx.cc.as_ref(), &b, &snapshot)
        .unwrap();
    assert!(matches!(outcome, GateOutcome::Refreshed(_)), "{outcome:?}");
}

#[test]
fn list_and_status_inside_a_run_shell_see_the_default_home() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let engine = fx.engine_located(fx.shell_env(&fx.make_profile(&a)));
    let lists = engine.accounts(None).unwrap();
    assert_eq!(lists[0].active_position, Some(2));
    let active: Vec<AccountId> = lists[0]
        .accounts
        .iter()
        .filter(|v| v.active)
        .map(|v| v.row.id.clone())
        .collect();
    assert_eq!(active, [b.clone()]);
    let StatusView::Managed { account, .. } = engine.status(&fx.provider()).unwrap() else {
        panic!("the default home's live login is managed")
    };
    assert_eq!(account.row.id, b);
}

#[test]
fn inside_a_run_shell_every_account_change_refuses_and_changes_nothing() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let engine = fx.engine_located(fx.shell_env(&fx.make_profile(&a)));
    assert!(matches!(engine.run_shell(), RunShell::Inside { .. }));
    for (command, got) in account_changes(&fx, &engine, &a) {
        // §12.8, B.57: the active-token refresh sees the default home, exactly as outside the
        // shell; the default login's token is valid, so it needs no request.
        let want = (command != "active refresh").then_some("inside-run-shell");
        assert_eq!(got, want, "{command}");
    }
    assert_eq!(token_requests(&fx), 0);
    let row = fx.engine.store().unwrap().account(&a).unwrap().unwrap();
    assert_eq!((row.position, row.alias, row.disabled), (1, None, false));
    assert_eq!(fx.live_email().as_deref(), Some("b@x.co"));
}

#[test]
fn under_an_unreadable_marker_every_account_change_refuses_naming_it() {
    // Review Focus 4: the outer home is unknown, so nothing acts on a guess.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let profile = fx.make_profile(&a);
    let marker = profile.join(MARKER_FILE);
    fs::write(&marker, "{\"format\": \"tagteam-profile\"").unwrap();
    let engine = fx.engine_located(fx.shell_env(&profile));
    assert!(
        matches!(engine.run_shell(), RunShell::Unreadable { marker: m, .. } if *m == marker),
        "{:?}",
        engine.run_shell()
    );
    for (command, got) in account_changes(&fx, &engine, &a) {
        assert_eq!(got, Some("run-shell-unreadable"), "{command}");
    }
    let err = engine.switch(fx.switch_request(&a, false)).unwrap_err();
    assert!(
        err.to_string().contains(&marker.display().to_string()),
        "{err}"
    );
}

#[test]
fn inside_a_run_shell_an_expired_default_live_token_is_refreshed_in_the_default_home() {
    // §12.8, B.57: the live login the active-token refresh reads, refreshes and protects is the
    // default home's, inside a run shell exactly as outside it. The session's own profile is
    // neither read for it nor written.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let profile = fx.make_profile(&a);
    let held = credential("a@x.co", "rt-a-session");
    fx.set_profile_credential(&profile, &held);
    let (svc, acct) = fx.profile_item(&profile);
    fx.kc.put(&svc, &acct, &held);
    let files = tree(&profile);
    let mut live = fx.live_credential().unwrap();
    live["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms());
    fx.set_live_credential(live.to_string().as_bytes());
    fx.script_refresh(Some("rt-b2"));
    let engine = fx.engine_located(fx.shell_env(&profile));
    assert!(matches!(engine.run_shell(), RunShell::Inside { .. }));

    let out = engine
        .refresh_active(&fx.provider(), ActiveTrigger::Expired)
        .unwrap();

    assert_eq!(out, ActiveOutcome::Refreshed);
    assert_eq!(sent_refresh_tokens(&fx), ["rt-b"]);
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b2"));
    assert_eq!(fx.vault_refresh_token(&b).as_deref(), Some("rt-b2"));
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(
        fx.kc.get(&svc, &acct).unwrap(),
        held,
        "the profile's item is untouched"
    );
    assert_eq!(tree(&profile), files, "and so are its files");
}
