//! §15.2 provider neutrality: the engine runs the test-only `FakeAgent` beside Claude Code,
//! through the same store, vault, switch transaction and local-state invariant.
mod common;

use std::fs;

use common::{FakeFx, Fx};
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::lifecycle::AddTokenOptions;
use tagteam_engine::switch::{SwitchRequest, SwitchTarget};
use tagteam_fake::{
    FAKE_AGENT, FakePaths, KIND_TOKEN, LOGIN_EXPIRES, credential_json, identity_json,
};
use tagteam_provider::http::Method;
use tagteam_provider::{Clock, IdentitySurface, Provider};

fn check(ffx: &FakeFx, surface: &IdentitySurface, step: &str, op: impl FnOnce()) {
    let before = ffx.fx.snapshot();
    op();
    let after = ffx.fx.snapshot();
    ffx.fx
        .assert_only_surface_changed_for(surface, &before, &after, step);
}

fn read_json(path: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn fake_agent_accounts_add_and_switch_through_the_engine() {
    let ffx = FakeFx::new();
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let bob = ffx.fake_add("bob", "tok-b", "renew-b");
    assert_eq!(ffx.fake_live_label().as_deref(), Some("bob@ws"));
    // The machine's device key moved on since alice's credential was stored.
    let p = FakePaths::resolve(&ffx.fx.env);
    let mut live = read_json(&p.credential);
    live["device"] = json!({"id": "machine-2"});
    fs::write(&p.credential, serde_json::to_vec(&live).unwrap()).unwrap();

    let out = ffx.switch_fake(&alice);
    assert!(out.switched, "{}", out.message);
    assert_eq!(out.from.map(|r| r.id), Some(bob.clone()));
    assert_eq!(ffx.fake_live_label().as_deref(), Some("alice@ws"));
    assert_eq!(
        read_json(&p.credential),
        json!({"fa": {"token": "tok-a", "renew": "renew-a", "expires": LOGIN_EXPIRES},
               "device": {"id": "machine-2"}})
    );
    let store = ffx.engine.store().unwrap();
    assert_eq!(store.active(&ffx.fake_provider()).unwrap(), Some(alice));
    assert!(ffx.switch_fake(&bob).switched);
    assert_eq!(ffx.fake_live_label().as_deref(), Some("bob@ws"));
}

#[test]
fn positions_are_numbered_per_provider() {
    let ffx = FakeFx::new();
    let cc = ffx.fx.add("a@b.co", "rt-a");
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let bob = ffx.fake_add("bob", "tok-b", "renew-b");
    let store = ffx.engine.store().unwrap();
    let pos = |id: &AccountId| store.account(id).unwrap().unwrap().position;
    assert_eq!((pos(&cc), pos(&alice), pos(&bob)), (1, 1, 2));
}

#[test]
fn accounts_lists_every_provider_with_accounts() {
    let ffx = FakeFx::new();
    ffx.fx.add("a@b.co", "rt-a");
    ffx.fake_add("alice", "tok-a", "renew-a");
    ffx.fake_add("bob", "tok-b", "renew-b");
    let lists = ffx.engine.accounts(None).unwrap();
    let summary: Vec<_> = lists
        .iter()
        .map(|l| (l.provider.as_str(), l.accounts.len(), l.active_position))
        .collect();
    assert_eq!(
        summary,
        vec![("claude-code", 1, Some(1)), (FAKE_AGENT, 2, Some(2))]
    );
    let fake_only = ffx.engine.accounts(Some(&ffx.fake_provider())).unwrap();
    assert_eq!(fake_only.len(), 1);
    assert_eq!(
        fake_only[0].accounts[0].kind,
        ffx.fake.kind_traits(KIND_TOKEN)
    );
}

#[test]
fn a_fake_agent_account_has_no_claude_code_shaped_field() {
    let ffx = FakeFx::new();
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let row = ffx
        .engine
        .store()
        .unwrap()
        .account(&alice)
        .unwrap()
        .unwrap();
    assert_eq!(row.provider.as_str(), FAKE_AGENT);
    assert_eq!(
        (
            row.label.as_str(),
            row.email.as_deref(),
            row.org_uuid.as_str()
        ),
        ("alice@ws", None, "ws")
    );
    assert_eq!(row.account_uuid.as_deref(), Some("uid-alice"));
    assert_eq!(row.kind, KIND_TOKEN);
    assert!(ffx.fake.credential_kinds().contains(&row.kind.as_str()));
    assert_eq!(row.identity_json, identity_json("alice", "ws", "uid-alice"));
    assert_eq!(row.login_expires_at, None);
    let stored: Value = serde_json::from_slice(&ffx.fx.vault_bytes(&alice).unwrap()).unwrap();
    assert_eq!(
        stored,
        credential_json("tok-a", Some("renew-a"), Some(LOGIN_EXPIRES))
    );
    assert!(stored.get("claudeAiOauth").is_none());
    assert!(row.identity_json.get("emailAddress").is_none());
}

#[test]
fn a_fake_agent_switch_leaves_claude_code_untouched() {
    let ffx = FakeFx::new();
    let _cc = ffx.fx.add("a@b.co", "rt-a");
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let _bob = ffx.fake_add("bob", "tok-b", "renew-b");
    let store = ffx.engine.store().unwrap();
    let cc_rows = store.accounts(&ffx.fx.provider()).unwrap();
    let cc_active = store.active(&ffx.fx.provider()).unwrap();
    let before = ffx.fx.snapshot();
    assert!(ffx.switch_fake(&alice).switched);
    let after = ffx.fx.snapshot();
    // With FakeAgent's surface, every Claude Code file and Keychain item must be unchanged.
    ffx.fx.assert_only_surface_changed_for(
        &ffx.fake.identity_surface(&ffx.fx.env),
        &before,
        &after,
        "a FakeAgent switch",
    );
    assert_eq!(store.accounts(&ffx.fx.provider()).unwrap(), cc_rows);
    assert_eq!(store.active(&ffx.fx.provider()).unwrap(), cc_active);
    assert_eq!(ffx.fx.live_email().as_deref(), Some("a@b.co"));
}

#[test]
fn a_claude_code_switch_leaves_fake_agent_untouched() {
    let ffx = FakeFx::new();
    let a = ffx.fx.add("a@b.co", "rt-a");
    let _b = ffx.fx.add("b@b.co", "rt-b");
    let _alice = ffx.fake_add("alice", "tok-a", "renew-a");
    let store = ffx.engine.store().unwrap();
    let fake_rows = store.accounts(&ffx.fake_provider()).unwrap();
    let fake_active = store.active(&ffx.fake_provider()).unwrap();
    let before = ffx.fx.snapshot();
    // Through the two-provider engine, so routing by provider is exercised too.
    assert!(
        ffx.engine
            .switch(ffx.fx.switch_request(&a, false))
            .unwrap()
            .switched
    );
    let after = ffx.fx.snapshot();
    // With Claude Code's surface, every FakeAgent file must be unchanged.
    ffx.fx
        .assert_only_surface_changed(&before, &after, "a Claude Code switch");
    assert_eq!(store.accounts(&ffx.fake_provider()).unwrap(), fake_rows);
    assert_eq!(store.active(&ffx.fake_provider()).unwrap(), fake_active);
    assert_eq!(ffx.fake_live_label().as_deref(), Some("alice@ws"));
    assert_eq!(ffx.fx.live_email().as_deref(), Some("a@b.co"));
}

#[test]
fn every_fake_agent_command_writes_only_its_identity_surface() {
    // §15.3, once per registered provider: FakeAgent's commands move only its declared
    // surface, and a Claude Code login beside it stays byte-identical throughout.
    let ffx = FakeFx::new();
    ffx.fx.add("cc@b.co", "rt-cc");
    let surface = ffx.fake.identity_surface(&ffx.fx.env);
    let e = &ffx.engine;
    let fake = ffx.fake_provider();

    ffx.fake_login("alice", "tok-a", "renew-a");
    let mut alice = None;
    check(&ffx, &surface, "add alice", || {
        alice = Some(e.add_live(ffx.fake_add_options()).unwrap().account.id)
    });
    let alice = alice.unwrap();
    ffx.fake_login("bob", "tok-b", "renew-b");
    let mut bob = None;
    check(&ffx, &surface, "add bob", || {
        bob = Some(e.add_live(ffx.fake_add_options()).unwrap().account.id)
    });
    let bob = bob.unwrap();
    check(&ffx, &surface, "switch to alice", || {
        assert!(ffx.switch_fake(&alice).switched)
    });
    check(&ffx, &surface, "alias", || {
        e.set_alias(&alice, Some("al")).unwrap();
    });
    check(&ffx, &surface, "disable", || {
        e.set_disabled(&bob, true).unwrap();
    });
    check(&ffx, &surface, "enable", || {
        e.set_disabled(&bob, false).unwrap();
    });
    check(&ffx, &surface, "move", || {
        e.move_to(&bob, 5).unwrap();
    });
    let mut token = None;
    check(&ffx, &surface, "add-token", || {
        token = Some(
            e.add_token(AddTokenOptions {
                provider: fake.clone(),
                token: "static-secret".into(),
                position: None,
                email: None,
                alias: None,
                yes: false,
            })
            .unwrap()
            .account,
        )
    });
    let token = token.unwrap();
    assert_eq!(
        token.label, "fa-static-6@token.local",
        "the provider's own prefix"
    );
    check(&ffx, &surface, "switch to the static token", || {
        assert!(ffx.switch_fake(&token.id).switched)
    });
    check(&ffx, &surface, "forced switch", || {
        let out = e
            .switch(SwitchRequest {
                provider: fake.clone(),
                target: SwitchTarget::Account(bob.clone()),
                force: true,
                source: "cli",
                auto: None,
            })
            .unwrap();
        assert!(out.switched);
    });
    check(&ffx, &surface, "remove", || {
        e.remove(&alice).unwrap();
    });
    assert_eq!(ffx.fake_live_label().as_deref(), Some("bob@ws"));
    assert_eq!(ffx.fx.live_email().as_deref(), Some("cc@b.co"));
}

/// `id`'s stored FakeAgent access token, moved inside the freshen window (§7.2).
fn make_due(ffx: &FakeFx, id: &AccountId) {
    let mut v: Value = serde_json::from_slice(&ffx.fx.vault_bytes(id).unwrap()).unwrap();
    v["fa"]["expires"] = json!(ffx.fx.clock.now_ms() + 60_000);
    ffx.fx.put_vault(id, v.to_string().as_bytes());
}

fn renew_requests(ffx: &FakeFx) -> usize {
    ffx.fx.http.count(Method::Post, &ffx.fake.renew_url())
}

#[test]
fn a_fake_agent_refresh_before_a_switch_leaves_claude_code_untouched() {
    // §15.2: a refresh on one provider never touches the other's state. The gate and freshen
    // path run FakeAgent's own endpoint and shapes, and only FakeAgent's surface moves.
    let ffx = FakeFx::new();
    ffx.fx.add("cc@b.co", "rt-cc");
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    ffx.fake_add("bob", "tok-b", "renew-b");
    make_due(&ffx, &alice);
    ffx.fx.http.push_json(
        Method::Post,
        &ffx.fake.renew_url(),
        200,
        json!({"token": "tok-a-2", "renew": "renew-a-2", "expires_in": 3600}),
    );
    let before = ffx.fx.snapshot();
    let out = ffx.switch_fake(&alice);
    let after = ffx.fx.snapshot();
    assert!(out.switched, "{}", out.message);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(renew_requests(&ffx), 1);
    let sent = ffx.fx.http.requests();
    let body: Value = serde_json::from_slice(sent[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(body, json!({"renew": "renew-a"}));
    let stored: Value = serde_json::from_slice(&ffx.fx.vault_bytes(&alice).unwrap()).unwrap();
    assert_eq!(stored["fa"]["renew"], "renew-a-2");
    assert_eq!(stored["fa"]["token"], "tok-a-2");
    let live = read_json(&FakePaths::resolve(&ffx.fx.env).credential);
    assert_eq!(
        live["fa"]["renew"], "renew-a-2",
        "the successor was activated"
    );
    assert_eq!(ffx.fake_live_label().as_deref(), Some("alice@ws"));
    ffx.fx.assert_only_surface_changed_for(
        &ffx.fake.identity_surface(&ffx.fx.env),
        &before,
        &after,
        "a FakeAgent refresh and switch",
    );
    assert_eq!(ffx.fx.live_email().as_deref(), Some("cc@b.co"));
    assert_eq!(
        ffx.fx.http.count(Method::Post, &Fx::endpoints().token),
        0,
        "nothing was sent to Claude Code's endpoint"
    );
}

#[test]
fn a_dead_fake_agent_target_is_quarantined_and_refused_without_touching_claude_code() {
    let ffx = FakeFx::new();
    ffx.fx.add("cc@b.co", "rt-cc");
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    ffx.fake_add("bob", "tok-b", "renew-b");
    make_due(&ffx, &alice);
    ffx.fx.http.push_json(
        Method::Post,
        &ffx.fake.renew_url(),
        400,
        json!({"error": "invalid_grant"}),
    );
    let before = ffx.fx.snapshot();
    let err = ffx
        .engine
        .switch(SwitchRequest {
            provider: ffx.fake_provider(),
            target: SwitchTarget::Account(alice.clone()),
            force: false,
            source: "cli",
            auto: None,
        })
        .unwrap_err();
    let after = ffx.fx.snapshot();
    assert_eq!(err.kind(), "relogin-required", "{err}");
    assert_eq!(renew_requests(&ffx), 1);
    let row = ffx
        .engine
        .store()
        .unwrap()
        .account(&alice)
        .unwrap()
        .unwrap();
    assert_eq!(row.quarantine_reason.as_deref(), Some("invalid_grant"));
    assert_eq!(
        ffx.fake_live_label().as_deref(),
        Some("bob@ws"),
        "nothing activated"
    );
    ffx.fx.assert_only_surface_changed_for(
        &ffx.fake.identity_surface(&ffx.fx.env),
        &before,
        &after,
        "a refused FakeAgent switch",
    );
    assert_eq!(ffx.fx.live_email().as_deref(), Some("cc@b.co"));
}

#[test]
fn the_freshen_warning_names_the_provider_that_will_refresh() {
    let ffx = FakeFx::new();
    let alice = ffx.fake_add("alice", "tok-a", "renew-a");
    ffx.fake_add("bob", "tok-b", "renew-b");
    make_due(&ffx, &alice);
    ffx.fx.http.push_json(
        Method::Post,
        &ffx.fake.renew_url(),
        400,
        json!({"error": "invalid_client"}),
    );
    let out = ffx.switch_fake(&alice);
    assert!(out.switched, "{}", out.message);
    assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
    assert!(
        out.warnings[0].ends_with("); FakeAgent will refresh it when it is online"),
        "{:?}",
        out.warnings
    );
}
