//! §15.3's local-state invariant around a whole `tagteam run` (§3, §12.4), for both providers,
//! and §15.2's fresh home. Launch and exit handling write nothing in the default home but the
//! `projects` and `mcpServers` subtrees, by merge, and the must-share entries, created empty.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{FakeFx, Fx, claude_bin};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::launch::LaunchEnd;
use tagteam_engine::store::AccountRow;
use tagteam_provider::process::Captured;
use tagteam_provider::splice::{get_top_level, replace_top_level};
use tagteam_provider::{IdentitySurface, Provider};

fn row(fx: &Fx, id: &AccountId) -> AccountRow {
    fx.engine.store().unwrap().account(id).unwrap().unwrap()
}

/// The directory `claude` runs in: outside HOME, so the snapshot never sees it.
fn work(dir: &Path) -> PathBuf {
    let work = dir.join("work");
    fs::create_dir_all(&work).unwrap();
    work
}

/// What `claude auth status --json` prints for `email`'s login in `id`'s profile: §12.3's
/// `valid` row. A first launch creates the profile under the canonical data dir, so that is its
/// spelling (§12.2 "One spelling").
fn valid_status(fx: &Fx, id: &AccountId, email: &str) -> Captured {
    let spelling = fs::canonicalize(fx.env.data_dir())
        .unwrap()
        .join("sessions")
        .join(id.as_str());
    Captured::Exited {
        code: Some(0),
        signal: None,
        stdout: json!({
            "loggedIn": true,
            "authMethod": "claude.ai",
            "apiProvider": "firstParty",
            "configDirectory": spelling.to_str().unwrap(),
            "email": email,
        })
        .to_string()
        .into_bytes(),
        stderr: vec![],
    }
}

/// `run`'s surface (§3): the identity surface plus the `projects` and `mcpServers` subtrees of
/// the global config, which a merge-back writes.
fn run_surface(fx: &Fx) -> IdentitySurface {
    let mut surface = fx.cc.identity_surface(&fx.env);
    let config = fx.paths().global_config;
    for (path, keys) in &mut surface.json_keys {
        if *path == config {
            keys.push("projects".into());
            keys.push("mcpServers".into());
        }
    }
    surface
}

/// FakeAgent's `run` surface (Decision 16): its identity surface plus the `prefs` key of the
/// outer `identity.json`, which its merge-back writes (Task 5).
fn fake_run_surface(ffx: &FakeFx) -> IdentitySurface {
    let mut surface = ffx.fake.identity_surface(&ffx.fx.env);
    for (path, keys) in &mut surface.json_keys {
        if path.file_name().is_some_and(|n| n == "identity.json") {
            keys.push("prefs".into());
        }
    }
    surface
}

/// Changes one top-level key of `file`, as the session's agent would.
fn edit_key(file: &Path, key: &str, edit: impl FnOnce(&mut Value)) {
    let doc = fs::read(file).unwrap();
    let mut value = get_top_level(&doc, key).unwrap().unwrap_or(Value::Null);
    edit(&mut value);
    fs::write(file, replace_top_level(&doc, key, &value).unwrap()).unwrap();
}

/// Changes one top-level key of `profile`'s `.claude.json`, as the session's Claude Code would.
fn edit_profile(profile: &Path, key: &str, edit: impl FnOnce(&mut Value)) {
    edit_key(&profile.join(".claude.json"), key, edit);
}

#[test]
fn a_run_merges_back_only_projects_and_mcp_servers_and_leaves_every_other_byte() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.spawner.push(valid_status(&fx, &a, "a@x.co"));
    let config = fx.paths().global_config;
    let before = fx.snapshot();

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &work(fx.dir.path()))
        .unwrap();
    let profile = launched.profile.clone();
    // The session trusts the project it runs in, opens another, adds a user-scope MCP server
    // and drops `local` (Appendix A.6). It also has a per-home id of its own.
    edit_profile(&profile, "projects", |p| {
        p["/work/app"]["hasTrustDialogAccepted"] = json!(true);
        p["/work/new"] = json!({"allowedTools": [], "hasTrustDialogAccepted": true});
    });
    edit_profile(&profile, "mcpServers", |m| {
        *m = json!({"remote": {"type": "http", "url": "https://mcp.example.com"}});
    });
    edit_profile(&profile, "userID", |u| *u = json!("profile-user"));
    fx.engine.finish_run(launched, LaunchEnd::Exited(0));
    let after = fx.snapshot();

    fx.assert_only_surface_changed_for(&run_surface(&fx), &before, &after, "run with merge-back");
    let doc = fs::read(&config).unwrap();
    assert_eq!(
        get_top_level(&doc, "projects").unwrap().unwrap(),
        json!({
            "/work/app": {"allowedTools": [], "history": ["x"], "hasTrustDialogAccepted": true},
            "/work/new": {"allowedTools": [], "hasTrustDialogAccepted": true}
        })
    );
    assert_eq!(
        get_top_level(&doc, "mcpServers").unwrap().unwrap(),
        json!({"remote": {"type": "http", "url": "https://mcp.example.com"}})
    );
    assert_eq!(
        get_top_level(&doc, "userID").unwrap().unwrap(),
        json!("user-7"),
        "per-home and account fields are never merged back (§12.4)"
    );
    assert!(
        !profile.join(".tagteam-baseline.json").exists(),
        "the merge-back ran"
    );
}

#[test]
fn a_run_whose_session_changed_nothing_leaves_the_default_config_byte_identical() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.spawner.push(valid_status(&fx, &a, "a@x.co"));
    let config = fx.paths().global_config;
    let bytes = fs::read(&config).unwrap();
    let before = fx.snapshot();

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &work(fx.dir.path()))
        .unwrap();
    fx.engine.finish_run(launched, LaunchEnd::Exited(0));
    let after = fx.snapshot();

    fx.assert_only_surface_changed(&before, &after, "run with nothing to merge back");
    assert_eq!(
        fs::read(&config).unwrap(),
        bytes,
        "not even `projects` or `mcpServers` is rewritten"
    );
}

#[test]
fn a_fake_agent_run_merges_back_only_its_prefs_and_writes_nothing_of_claude_code_s() {
    // §15.2 "Provider neutrality": the snapshot walks all of HOME and every Keychain item, so
    // FakeAgent's surface also proves Claude Code's home untouched. FakeAgent validates from
    // its profile files, so the launch spawns nothing (Task 7).
    let ffx = FakeFx::new();
    let h1 = ffx.fake_add("h1", "tok-1", "renew-1");
    ffx.fake_add("h2", "tok-2", "renew-2");
    let row = ffx.engine.store().unwrap().account(&h1).unwrap().unwrap();
    let surface = fake_run_surface(&ffx);
    let outer = surface.json_keys[0].0.clone();
    let before = ffx.fx.snapshot();

    let launched = ffx
        .engine
        .launch(&row, claude_bin(), &work(ffx.fx.dir.path()))
        .unwrap();
    // The session sets a pref of its own (Task 5's `prefs.<name>`).
    edit_key(&launched.profile.join("identity.json"), "prefs", |p| {
        p["lang"] = json!("nl");
    });
    ffx.engine.finish_run(launched, LaunchEnd::Exited(0));
    let after = ffx.fx.snapshot();

    ffx.fx
        .assert_only_surface_changed_for(&surface, &before, &after, "a FakeAgent run");
    let prefs = get_top_level(&fs::read(&outer).unwrap(), "prefs")
        .unwrap()
        .unwrap();
    assert_eq!(
        prefs["lang"],
        json!("nl"),
        "merged back into the outer identity.json"
    );
}

#[test]
fn a_fresh_home_gets_its_memory_and_history_created_empty_and_shared() {
    // §15.2 "Fresh home", Review Focus 5's first half, at the level of a whole launch.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let claude = fx.env.home.join(".claude");
    fs::remove_dir_all(claude.join("projects")).unwrap();
    fs::remove_file(claude.join("history.jsonl")).unwrap();
    fx.spawner.push(valid_status(&fx, &a, "a@x.co"));
    let before = fx.snapshot();

    let launched = fx
        .engine
        .launch(&row(&fx, &a), claude_bin(), &work(fx.dir.path()))
        .unwrap();
    let profile = launched.profile.clone();
    fx.engine.finish_run(launched, LaunchEnd::Exited(0));
    let after = fx.snapshot();

    fx.assert_only_surface_changed(&before, &after, "a fresh home's first run");
    assert_eq!(fs::read_dir(claude.join("projects")).unwrap().count(), 0);
    assert_eq!(fs::read(claude.join("history.jsonl")).unwrap(), b"");
    for name in ["projects", "history.jsonl"] {
        let link = profile.join(name);
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "{name} is shared, never a private copy (§12.2, B.43)"
        );
        assert_eq!(
            fs::read_link(&link).unwrap(),
            fs::canonicalize(claude.join(name)).unwrap()
        );
    }
}
