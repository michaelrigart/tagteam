mod common;

use std::any::Any;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{
    Fx, HomeSnapshot, assert_journal_cleared, crashed_switch, splice_config_key,
    write_target_credential,
};
use serde_json::json;
use tagteam_cc::live::Platform;
use tagteam_engine::switch::{SwitchOutcome, SwitchRequest};

fn check(fx: &Fx, step: &str, op: impl FnOnce()) {
    let before = fx.snapshot();
    op();
    let after = fx.snapshot();
    fx.assert_only_surface_changed(&before, &after, step);
}

/// The text of a caught panic payload, whether it was raised via `panic!("...")` (`&str`) or
/// `format!` inside `assert!`/`assert_eq!` (`String`).
fn panic_text(payload: Box<dyn Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

/// Runs the comparison over `before`/`after`, asserting it panics and that its message
/// contains `needle` — never just that it panicked, so a negative test can't pass on the
/// comparison failing for an unrelated reason. Returns the message for a caller that wants to
/// check more than one substring.
fn expect_violation(
    fx: &Fx,
    before: &HomeSnapshot,
    after: &HomeSnapshot,
    step: &str,
    needle: &str,
) -> String {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fx.assert_only_surface_changed(before, after, step)
    }));
    let msg = panic_text(r.expect_err("expected the comparison to panic"));
    assert!(
        msg.contains(needle),
        "panic message {msg:?} did not contain {needle:?}"
    );
    msg
}

/// A relative path from directory `from` to `to`, for building a relative symlink target — the
/// shape GNU stow creates.
fn relative_from(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for comp in &to[common..] {
        out.push(comp.as_os_str());
    }
    out
}

/// Moves `real` into `home/dotfiles/<name>` and replaces it with a symlink to there — absolute,
/// or relative (the GNU-stow shape) when `relative` — without duplicating the setup in every
/// test.
fn symlink_into_dotfiles(home: &Path, real: &Path, relative: bool) {
    let dot = home.join("dotfiles");
    std::fs::create_dir_all(&dot).unwrap();
    let target = dot.join(real.file_name().unwrap());
    std::fs::rename(real, &target).unwrap();
    let link_target = if relative {
        relative_from(real.parent().unwrap(), &target)
    } else {
        target
    };
    std::os::unix::fs::symlink(&link_target, real).unwrap();
}

/// A starting state closer to a real `~/.claude`, so the invariant exercises the masking and
/// preservation rules against genuine noise instead of an empty slate: a pre-existing
/// `customApiKeyResponses` with both `approved` and `rejected` entries, a non-default file mode
/// that must survive untouched, and an unrelated Keychain item that must stay untouched.
fn seed_realistic_state(fx: &Fx) {
    splice_config_key(
        &fx.paths().global_config,
        "customApiKeyResponses",
        &json!({"approved": ["existing-approved-suffix0"], "rejected": ["existing-rejected-suffix0"]}),
    );
    std::fs::set_permissions(
        fx.env.home.join(".claude/CLAUDE.md"),
        std::fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    fx.kc.put("Other Service", "someone", b"unrelated");
}

/// Further machine-shared keys beyond the `mcpOAuth` every other test already exercises, plus
/// an unknown sibling key: the masking rule must generalize to the whole declared set and must
/// not require preserving anything outside it. Requires a live credential to already exist.
fn seed_extra_credential_keys(fx: &Fx) {
    let mut v = fx.live_credential().unwrap();
    v["mcpOAuthClientConfig"] = json!({"client": "shared-config"});
    v["pluginSecrets"] = json!({"plugin": "shared-secret"});
    v["someFutureCredentialKey"] = json!({"unknown": true});
    fx.set_live_credential(v.to_string().as_bytes());
}

/// A login that also re-seeds the extra machine-shared keys `seed_extra_credential_keys`
/// plants: `Fx::login` replaces the whole live credential with a fresh one carrying only
/// `mcpOAuth`, so every login in the sequence must re-apply the extras or they vanish after the
/// very next one (fix round 2, finding 1).
fn login_with_extras(fx: &Fx, email: &str, rt: &str) {
    fx.login(email, rt);
    seed_extra_credential_keys(fx);
}

/// Asserts the extra machine-shared keys `seed_extra_credential_keys` planted are still present
/// in the live credential — a positive check, on top of the invariant, that a switch actually
/// carries the whole declared machine-shared set forward, not just the `mcpOAuth` every other
/// scenario already happens to exercise.
fn assert_extra_credential_keys_present(fx: &Fx) {
    let v = fx
        .live_credential()
        .expect("a live credential must exist once any account has logged in");
    assert_eq!(
        v.get("mcpOAuthClientConfig"),
        Some(&json!({"client": "shared-config"})),
        "mcpOAuthClientConfig did not survive the switch"
    );
    assert_eq!(
        v.get("pluginSecrets"),
        Some(&json!({"plugin": "shared-secret"})),
        "pluginSecrets did not survive the switch"
    );
}

/// Switches, then asserts the switch actually took effect by comparing the live identity to
/// the outcome's own declared target — never a hard-coded email — so a switch that silently
/// no-ops can never hide behind an invariant check that only looks for illegal writes. Also
/// asserts the extra machine-shared keys survived, on top of the invariant's own check.
fn switch_and_verify(fx: &Fx, req: SwitchRequest) -> SwitchOutcome {
    let outcome = fx.engine.switch(req).unwrap();
    assert!(outcome.switched, "switch did not report switched");
    let to = outcome
        .to
        .as_ref()
        .expect("a switch outcome must always resolve a target");
    assert_eq!(
        fx.live_email(),
        to.email.clone(),
        "switch to {:?} did not take effect",
        to.id
    );
    assert_extra_credential_keys_present(fx);
    outcome
}

fn run_every_command(platform: Platform) {
    run_every_command_on(&Fx::with_platform(platform));
}

fn run_every_command_on(fx: &Fx) {
    seed_realistic_state(fx);

    login_with_extras(fx, "a@x.co", "rt-a");
    let mut a = None;
    check(fx, "add a", || {
        a = Some(fx.engine.add_live(fx.add_options()).unwrap().account.id)
    });
    let a = a.unwrap();

    login_with_extras(fx, "b@x.co", "rt-b");
    let mut b = None;
    check(fx, "add b", || {
        b = Some(fx.engine.add_live(fx.add_options()).unwrap().account.id)
    });
    let b = b.unwrap();

    let mut k = None;
    check(fx, "add-token", || {
        k = Some(
            fx.engine
                .add_token(fx.add_token_options("sk-ant-api03-invariant-key-000000000"))
                .unwrap()
                .account
                .id,
        )
    });
    let k = k.unwrap();

    let mut s = None;
    check(fx, "add-token setup", || {
        s = Some(
            fx.engine
                .add_token(fx.add_token_options("sk-ant-oat01-setup-token-invariant"))
                .unwrap()
                .account
                .id,
        )
    });
    let s = s.unwrap();

    check(fx, "switch b→a", || {
        drop(switch_and_verify(fx, fx.switch_request(&a, false)))
    });
    fx.rotate_live("rt-a2"); // CC refreshes while a is live
    check(fx, "switch a→api key", || {
        drop(switch_and_verify(fx, fx.switch_request(&k, false)))
    });
    check(fx, "switch api key→b", || {
        drop(switch_and_verify(fx, fx.switch_request(&b, false)))
    });
    check(fx, "switch b→setup token", || {
        drop(switch_and_verify(fx, fx.switch_request(&s, false)))
    });
    check(fx, "switch setup token→b", || {
        drop(switch_and_verify(fx, fx.switch_request(&b, false)))
    });
    check(fx, "forced self-switch", || {
        drop(switch_and_verify(fx, fx.switch_request(&b, true)))
    });
    check(fx, "rotation", || {
        drop(switch_and_verify(fx, fx.rotation_request(false)))
    });
    check(fx, "alias", || {
        drop(fx.engine.set_alias(&a, Some("work")).unwrap())
    });
    check(fx, "disable", || {
        drop(fx.engine.set_disabled(&a, true).unwrap())
    });
    check(fx, "enable", || {
        drop(fx.engine.set_disabled(&a, false).unwrap())
    });
    check(fx, "move", || drop(fx.engine.move_to(&a, 3).unwrap()));

    // Interrupted-switch recovery: a journal row held by a dead process, recovered on the next
    // mutation (§15.3's coverage floor: recovery is itself a mutating code path).
    check(fx, "switch before recovery", || {
        drop(switch_and_verify(fx, fx.switch_request(&b, false)))
    });
    crashed_switch(fx, &b, &a);
    write_target_credential(fx, &a);
    check(fx, "recover interrupted switch", || {
        drop(fx.engine.set_disabled(&a, false).unwrap());
        assert_eq!(
            fx.live_email().as_deref(),
            Some("a@x.co"),
            "forward recovery did not land"
        );
    });
    assert_journal_cleared(fx);

    // A forced switch that displaces an unmanaged login: CC was logged into an identity
    // tagteam never captured, and the forced switch must still land.
    login_with_extras(fx, "stranger@x.co", "rt-stranger");
    check(fx, "forced switch displaces unmanaged login", || {
        drop(switch_and_verify(fx, fx.switch_request(&b, true)))
    });

    // `remove` of the live account: §10.3 never touches the live login, so this should be a
    // no-op on the identity surface even though it deletes tagteam's own row for it.
    check(fx, "switch to a before live removal", || {
        drop(switch_and_verify(fx, fx.switch_request(&a, false)))
    });
    check(fx, "remove live account", || {
        drop(fx.engine.remove(&a).unwrap())
    });

    check(fx, "accounts", || drop(fx.engine.accounts(None).unwrap()));
    check(fx, "status", || {
        drop(fx.engine.status(&fx.provider()).unwrap())
    });
    check(fx, "statusline", || {
        drop(fx.engine.statusline(&fx.provider()).unwrap())
    });

    check(fx, "remove", || drop(fx.engine.remove(&k).unwrap()));
}

#[test]
fn every_m1_command_writes_only_the_identity_surface_on_macos() {
    run_every_command(Platform::MacOs);
}

#[test]
fn every_m1_command_writes_only_the_identity_surface_on_linux() {
    run_every_command(Platform::Linux);
}

#[test]
fn every_m1_command_writes_only_the_identity_surface_through_symlinked_config() {
    // `~/.claude.json` (relative, GNU-stow style) and `~/.claude/settings.json` (absolute) are
    // both symlinked into a dotfiles-style location: the surface's write-through must apply to
    // the resolved target, not the link's own literal name, and the settings.json link (outside
    // the surface entirely) must stay completely untouched through the whole run (§15.3's
    // resolved-path fix).
    let fx = Fx::with_platform(Platform::MacOs);
    symlink_into_dotfiles(&fx.env.home, &fx.paths().global_config, true);
    symlink_into_dotfiles(
        &fx.env.home,
        &fx.env.home.join(".claude/settings.json"),
        false,
    );
    run_every_command_on(&fx);
    assert!(
        std::fs::symlink_metadata(&fx.paths().global_config)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the ~/.claude.json link itself must survive as a link"
    );
    assert!(
        std::fs::symlink_metadata(fx.env.home.join(".claude/settings.json"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the ~/.claude/settings.json link itself must survive as a link"
    );
}

#[test]
fn every_m1_command_writes_only_the_identity_surface_through_symlinked_config_on_linux() {
    // Linux is the platform where a secret-file write could plausibly replace a link outright
    // (an atomic rename onto the literal path rather than through it) — `write_atomic_with`
    // resolves through the link first, so both links (one absolute, one relative) must still
    // survive a full run.
    let fx = Fx::with_platform(Platform::Linux);
    std::fs::write(&fx.paths().credentials_file, b"{}").unwrap(); // must pre-exist to symlink
    symlink_into_dotfiles(&fx.env.home, &fx.paths().global_config, false);
    symlink_into_dotfiles(&fx.env.home, &fx.paths().credentials_file, true);
    run_every_command_on(&fx);
    assert!(
        std::fs::symlink_metadata(&fx.paths().global_config)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the ~/.claude.json link itself must survive as a link"
    );
    assert!(
        std::fs::symlink_metadata(&fx.paths().credentials_file)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the credentials-file link itself must survive as a link"
    );
}

#[test]
fn inert_former_fallback_items_stay_byte_identical() {
    // An explicit CLAUDE_CONFIG_DIR=~/.claude names only the suffixed items (Appendix A.2), so
    // the surface lists those alone, and the unsuffixed items fall under the byte-for-byte
    // rule: any command that read-modified-wrote one would be flagged.
    let fx = Fx::with_explicit_default_config_dir();
    fx.put_inert_items();
    run_every_command_on(&fx);
}

#[test]
fn the_comparison_catches_a_stray_write() {
    let fx = Fx::new();
    let before = fx.snapshot();
    std::fs::write(fx.env.home.join(".claude/CLAUDE.md"), "changed\n").unwrap();
    let after = fx.snapshot();
    expect_violation(&fx, &before, &after, "stray", "CLAUDE.md");
}

#[test]
fn the_comparison_catches_a_mode_change() {
    let fx = Fx::new();
    let p = fx.env.home.join(".claude/settings.json");
    let before = fx.snapshot();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    let after = fx.snapshot();
    expect_violation(&fx, &before, &after, "mode", "settings.json");
}

#[test]
fn the_comparison_catches_a_symlink_replaced_by_a_regular_file() {
    let fx = Fx::new();
    let p = fx.env.home.join(".claude/settings.json");
    symlink_into_dotfiles(&fx.env.home, &p, false);
    let target = fx.env.home.join("dotfiles/settings.json");
    let before = fx.snapshot();
    // Same bytes, but no longer a link: a byte-only comparison would miss this entirely.
    let bytes = std::fs::read(&target).unwrap();
    std::fs::remove_file(&p).unwrap();
    std::fs::write(&p, bytes).unwrap();
    let after = fx.snapshot();
    expect_violation(&fx, &before, &after, "swap", "settings.json");
}

#[test]
fn the_comparison_catches_a_new_directory() {
    let fx = Fx::new();
    let before = fx.snapshot();
    std::fs::create_dir_all(fx.env.home.join(".claude/agents/new")).unwrap();
    let after = fx.snapshot();
    expect_violation(&fx, &before, &after, "new-dir", "agents");
}

#[test]
fn the_comparison_catches_a_json_key_outside_the_surface() {
    let fx = Fx::new();
    let p = fx.paths().global_config;
    let before = fx.snapshot();
    splice_config_key(&p, "numStartups", &json!(13));
    let after = fx.snapshot();
    expect_violation(&fx, &before, &after, "json-key", "outside");
}

#[test]
fn the_comparison_catches_a_keychain_item_outside_the_surface() {
    let fx = Fx::new();
    fx.kc.put("Other", "tester", b"x");
    let before = fx.snapshot();
    fx.kc.put("Other", "tester", b"y");
    let after = fx.snapshot();
    expect_violation(&fx, &before, &after, "keychain-item", "Keychain item");
}

#[test]
fn the_comparison_catches_a_machine_shared_key_change_in_the_keychain_item() {
    let fx = Fx::new(); // macOS: the live credential is a Keychain item.
    fx.login("a@x.co", "rt-a");
    let before = fx.snapshot();
    let mut v = fx.live_credential().unwrap();
    v["mcpOAuth"] = json!({"srv": {"token": "tampered"}});
    fx.set_live_credential(v.to_string().as_bytes());
    let after = fx.snapshot();
    expect_violation(
        &fx,
        &before,
        &after,
        "shared-keychain",
        "machine-shared keys changed",
    );
}

#[test]
fn the_comparison_catches_a_machine_shared_key_change_in_credentials_json() {
    let fx = Fx::with_platform(Platform::Linux); // the live credential is a plain file.
    fx.login("a@x.co", "rt-a");
    let before = fx.snapshot();
    let mut v = fx.live_credential().unwrap();
    v["mcpOAuth"] = json!({"srv": {"token": "tampered"}});
    fx.set_live_credential(v.to_string().as_bytes());
    let after = fx.snapshot();
    expect_violation(
        &fx,
        &before,
        &after,
        "shared-file",
        "machine-shared keys changed",
    );
}

#[test]
fn the_comparison_allows_only_appends_to_approved_api_keys() {
    let seed = |fx: &Fx| {
        splice_config_key(
            &fx.paths().global_config,
            "customApiKeyResponses",
            &json!({"approved": ["a"], "rejected": ["r"]}),
        );
    };
    let fx = Fx::new();

    seed(&fx);
    let before = fx.snapshot();
    splice_config_key(
        &fx.paths().global_config,
        "customApiKeyResponses",
        &json!({"approved": ["a", "b"], "rejected": ["r"]}),
    );
    let after = fx.snapshot();
    fx.assert_only_surface_changed(&before, &after, "api-keys append"); // must not panic

    seed(&fx);
    let before = fx.snapshot();
    splice_config_key(
        &fx.paths().global_config,
        "customApiKeyResponses",
        &json!({"approved": [], "rejected": ["r"]}),
    );
    let after = fx.snapshot();
    expect_violation(
        &fx,
        &before,
        &after,
        "api-keys removal",
        "lost or reordered",
    );

    seed(&fx);
    let before = fx.snapshot();
    splice_config_key(
        &fx.paths().global_config,
        "customApiKeyResponses",
        &json!({"approved": ["a"], "rejected": []}),
    );
    let after = fx.snapshot();
    expect_violation(
        &fx,
        &before,
        &after,
        "api-keys rejected-touched",
        "beyond appending",
    );
}
