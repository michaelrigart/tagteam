mod common;

use common::Fx;
use tagteam_cc::live::Platform;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::switch::{SwitchRequest, SwitchTarget};

fn check(fx: &Fx, step: &str, op: impl FnOnce()) {
    let before = fx.snapshot();
    op();
    let after = fx.snapshot();
    fx.assert_only_surface_changed(&before, &after, step);
}

fn run_every_command(platform: Platform) {
    run_every_command_on(&Fx::with_platform(platform));
}

fn run_every_command_on(fx: &Fx) {
    let add = || AddOptions {
        provider: fx.provider(),
        position: None,
        alias: None,
        yes: false,
    };
    let sw = |id: &tagteam_core::AccountId, force| SwitchRequest {
        provider: fx.provider(),
        target: SwitchTarget::Account(id.clone()),
        force,
        source: "cli",
    };

    fx.login("a@x.co", "rt-a");
    let mut a = None;
    check(fx, "add a", || {
        a = Some(fx.engine.add_live(add()).unwrap().account.id)
    });
    let a = a.unwrap();
    fx.login("b@x.co", "rt-b");
    let mut b = None;
    check(fx, "add b", || {
        b = Some(fx.engine.add_live(add()).unwrap().account.id)
    });
    let b = b.unwrap();
    let mut k = None;
    check(fx, "add-token", || {
        k = Some(
            fx.engine
                .add_token(AddTokenOptions {
                    provider: fx.provider(),
                    token: "sk-ant-api03-invariant-key-000000000".into(),
                    position: None,
                    email: None,
                    alias: None,
                    yes: false,
                })
                .unwrap()
                .account
                .id,
        )
    });
    let k = k.unwrap();

    check(fx, "switch b→a", || {
        drop(fx.engine.switch(sw(&a, false)).unwrap())
    });
    fx.rotate_live("rt-a2"); // CC refreshes while a is live
    check(fx, "switch a→api key", || {
        drop(fx.engine.switch(sw(&k, false)).unwrap())
    });
    check(fx, "switch api key→b", || {
        drop(fx.engine.switch(sw(&b, false)).unwrap())
    });
    check(fx, "forced self-switch", || {
        drop(fx.engine.switch(sw(&b, true)).unwrap())
    });
    check(fx, "rotation", || {
        drop(
            fx.engine
                .switch(SwitchRequest {
                    provider: fx.provider(),
                    target: SwitchTarget::Rotation,
                    force: false,
                    source: "cli",
                })
                .unwrap(),
        )
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
fn fallback_keychain_items_stay_within_the_surface() {
    // An explicit CLAUDE_CONFIG_DIR=~/.claude: readers also try the unsuffixed items, so a
    // switch may touch them, and the surface must say so.
    let fx = Fx::with(Platform::MacOs, |e| {
        e.claude_config_dir = Some(e.home.join(".claude").into_os_string())
    });
    let acct = tagteam_cc::keychain_account(&fx.env);
    fx.kc.put(
        "Claude Code-credentials",
        &acct,
        br#"{"mcpOAuth":{"fallback":1}}"#,
    );
    run_every_command_on(&fx);
}

#[test]
fn the_comparison_catches_a_stray_write() {
    let fx = Fx::new();
    let before = fx.snapshot();
    std::fs::write(fx.env.home.join(".claude/CLAUDE.md"), "changed\n").unwrap();
    let after = fx.snapshot();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fx.assert_only_surface_changed(&before, &after, "stray")
    }));
    assert!(r.is_err());
}

#[test]
fn the_comparison_allows_only_appends_to_approved_api_keys() {
    let set = |fx: &Fx, v: serde_json::Value| {
        let path = fx.paths().global_config;
        let doc = std::fs::read(&path).unwrap();
        std::fs::write(
            &path,
            tagteam_provider::splice::replace_top_level(&doc, "customApiKeyResponses", &v).unwrap(),
        )
        .unwrap();
    };
    let caught = |fx: &Fx, after_value: serde_json::Value| {
        set(
            fx,
            serde_json::json!({"approved": ["a"], "rejected": ["r"]}),
        );
        let before = fx.snapshot();
        set(fx, after_value);
        let after = fx.snapshot();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fx.assert_only_surface_changed(&before, &after, "api-keys")
        }))
        .is_err()
    };
    let fx = Fx::new();
    assert!(
        !caught(
            &fx,
            serde_json::json!({"approved": ["a", "b"], "rejected": ["r"]})
        ),
        "an append is allowed"
    );
    assert!(
        caught(&fx, serde_json::json!({"approved": [], "rejected": ["r"]})),
        "a removal is not"
    );
    assert!(
        caught(&fx, serde_json::json!({"approved": ["a"], "rejected": []})),
        "touching rejected is not"
    );
}
