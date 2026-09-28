mod common;

use common::Fx;
use serde_json::json;
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_provider::Provider;
use tagteam_provider::splice::replace_top_level;

fn add_opts(fx: &Fx) -> AddOptions {
    AddOptions {
        provider: fx.provider(),
        position: None,
        alias: None,
        yes: false,
    }
}

fn token_opts(fx: &Fx, token: &str) -> AddTokenOptions {
    AddTokenOptions {
        provider: fx.provider(),
        token: token.into(),
        position: None,
        email: None,
        alias: None,
        yes: false,
    }
}

#[test]
fn add_captures_the_live_login() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let out = fx
        .engine
        .add_live(AddOptions {
            alias: Some("Work".into()),
            ..add_opts(&fx)
        })
        .unwrap();
    let a = &out.account;
    assert!(out.created);
    assert_eq!(
        (a.position, a.label.as_str(), a.kind.as_str()),
        (1, "me@work.co", "oauth")
    );
    assert_eq!(a.alias.as_deref(), Some("work"));
    assert_eq!(a.account_uuid.as_deref(), Some("uuid-me@work.co"));
    assert_eq!(a.login_expires_at, Some(1_797_000_000_000));
    assert_eq!(fx.vault_refresh_token(&a.id).as_deref(), Some("rt-1"));
    assert_eq!(
        fx.engine.store().unwrap().active(&fx.provider()).unwrap(),
        Some(a.id.clone())
    );
    assert!(
        out.notices.iter().any(|n| n.contains("could not verify")),
        "M1 has no oracle"
    );
}

#[test]
fn adding_the_same_login_again_refreshes_it_in_place() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let first = fx.engine.add_live(add_opts(&fx)).unwrap().account;
    fx.rotate_live("rt-2");
    let again = fx.engine.add_live(add_opts(&fx)).unwrap();
    assert!(!again.created);
    assert_eq!(again.account.id, first.id);
    assert_eq!(again.account.login_epoch, first.login_epoch + 1);
    assert_eq!(fx.vault_refresh_token(&first.id).as_deref(), Some("rt-2"));
    let prev = fx.kc.get("tagteam", &format!("{}.prev", first.id)).unwrap();
    assert!(String::from_utf8(prev).unwrap().contains("rt-1"));
}

#[test]
fn add_refuses_what_it_cannot_safely_capture() {
    let fx = Fx::new();
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::NoLiveLogin)
    ));

    fx.login("me@work.co", "rt-1");
    let (svc, acct) = (
        keychain_service(&fx.env, ItemKind::OAuth),
        keychain_account(&fx.env),
    );
    fx.kc.set_unreadable(&svc, &acct, true);
    std::fs::write(
        fx.paths().credentials_file,
        Fx::credential_json("me@work.co", "rt-0").to_string(),
    )
    .unwrap();
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::DegradedRead)
    ));
    fx.kc.set_unreadable(&svc, &acct, false);

    fx.kc.put(
        &keychain_service(&fx.env, ItemKind::ManagedKey),
        &acct,
        b"sk-ant-api03-live",
    );
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::LiveApiKey)
    ));
    assert!(
        fx.engine
            .existing_store()
            .unwrap()
            .is_none_or(|s| s.accounts(&fx.provider()).unwrap().is_empty())
    );
}

#[test]
fn add_refuses_a_wiped_live_credential() {
    // Task 13's review: `shape::classify` falls through to `setup_token` for any bytes it
    // does not recognise, wiped ones included, so a wiped live credential must be refused
    // before it ever reaches `classify` — and it must create nothing (§5).
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    fx.set_live_credential(
        json!({"claudeAiOauth": {"accessToken": "", "refreshToken": ""}})
            .to_string()
            .as_bytes(),
    );
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::NoLiveLogin)
    ));
    assert!(
        fx.engine
            .existing_store()
            .unwrap()
            .is_none_or(|s| s.accounts(&fx.provider()).unwrap().is_empty())
    );
    assert!(
        fx.vault_bytes(&tagteam_core::AccountId::from_string("anything"))
            .is_none()
    );
}

#[test]
fn an_oracle_naming_someone_else_refuses_the_add() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let other = fx
        .cc
        .parse_identity(&json!({"emailAddress": "other@x.co", "accountUuid": "uuid-other"}))
        .unwrap();
    fx.oracle.set(Some(other));
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::OwnerMismatch { .. })
    ));
    let me = fx
        .cc
        .parse_identity(&Fx::oauth_account("me@work.co"))
        .unwrap();
    fx.oracle.set(Some(me));
    let out = fx.engine.add_live(add_opts(&fx)).unwrap();
    assert!(out.notices.is_empty());
}

#[test]
fn an_occupied_position_needs_confirmation_and_then_replaces_its_occupant() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("b@x.co", "rt-b");
    let at1 = AddOptions {
        position: Some(1),
        ..add_opts(&fx)
    };
    assert!(matches!(
        fx.engine.add_live(at1),
        Err(EngineError::NeedsConfirmation { position: 1, .. })
    ));
    let b = fx
        .engine
        .add_live(AddOptions {
            position: Some(1),
            yes: true,
            ..add_opts(&fx)
        })
        .unwrap()
        .account;
    assert_eq!(b.position, 1);
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
    assert!(fx.vault_bytes(&a).is_none());
}

#[test]
fn a_replacement_that_cannot_be_written_keeps_its_occupant() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("b@x.co", "rt-b");
    fx.engine
        .add_live(AddOptions {
            alias: Some("work".into()),
            ..add_opts(&fx)
        })
        .unwrap();
    fx.login("c@x.co", "rt-c");
    // The alias belongs to someone else: refused before position 1 is touched.
    let clash = AddOptions {
        position: Some(1),
        yes: true,
        alias: Some("work".into()),
        ..add_opts(&fx)
    };
    assert!(matches!(
        fx.engine.add_live(clash),
        Err(EngineError::InvalidInput(_))
    ));
    // The vault refuses the new credential: the occupant survives.
    fx.kc.set_fail_write("tagteam", true);
    assert!(
        fx.engine
            .add_live(AddOptions {
                position: Some(1),
                yes: true,
                ..add_opts(&fx)
            })
            .is_err()
    );
    fx.kc.set_fail_write("tagteam", false);
    let store = fx.engine.store().unwrap();
    assert_eq!(store.account(&a).unwrap().unwrap().position, 1);
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"));
    assert_eq!(store.accounts(&fx.provider()).unwrap().len(), 2);
}

#[test]
fn an_unreadable_managed_key_refuses_the_add() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let (svc, acct) = (
        keychain_service(&fx.env, ItemKind::ManagedKey),
        keychain_account(&fx.env),
    );
    fx.kc.put(&svc, &acct, b"sk-ant-api03-live");
    fx.kc.set_unreadable(&svc, &acct, true);
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::Unreadable(_))
    ));
}

#[test]
fn adding_a_login_updates_an_account_of_another_kind_in_place() {
    // §10.1: an existing (email, org) is refreshed in place, its kind included.
    let fx = Fx::new();
    let token = AddTokenOptions {
        email: Some("me@work.co".into()),
        ..token_opts(&fx, "sk-ant-api03-k")
    };
    let before = fx.engine.add_token(token).unwrap().account;
    fx.login("me@work.co", "rt-1");
    let after = fx.engine.add_live(add_opts(&fx)).unwrap();
    assert!(!after.created);
    assert_eq!(
        (after.account.id, after.account.kind.as_str()),
        (before.id, "oauth")
    );
    assert_eq!(
        after.account.account_uuid.as_deref(),
        Some("uuid-me@work.co")
    );
}

#[test]
fn add_refuses_a_live_login_whose_account_uuid_conflicts_with_the_stored_one() {
    // `update_login`/`finish_replacement` COALESCE a new account_uuid over a known one, so a
    // stored uuid that would be overwritten by a *different* one must be refused rather than
    // silently clobbered.
    let fx = Fx::new();
    let a = fx.add("me@work.co", "rt-1");
    let identity = json!({
        "emailAddress": "me@work.co", "organizationUuid": "", "organizationName": null,
        "accountUuid": "uuid-different"
    });
    let doc = std::fs::read(&fx.paths().global_config).unwrap();
    std::fs::write(
        &fx.paths().global_config,
        replace_top_level(&doc, "oauthAccount", &identity).unwrap(),
    )
    .unwrap();
    fx.set_live_credential(
        Fx::credential_json("me@work.co", "rt-2")
            .to_string()
            .as_bytes(),
    );
    assert!(matches!(
        fx.engine.add_live(add_opts(&fx)),
        Err(EngineError::InvalidInput(_))
    ));
    let store = fx.engine.store().unwrap();
    assert_eq!(
        store.account(&a).unwrap().unwrap().account_uuid.as_deref(),
        Some("uuid-me@work.co")
    );
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-1"));
}

#[test]
fn add_token_rechecks_the_kind_after_a_pending_replacement_lands() {
    let fx = Fx::new();
    let token = |t: &str| AddTokenOptions {
        email: Some("me@work.co".into()),
        ..token_opts(&fx, t)
    };
    let x = fx
        .engine
        .add_token(token("sk-ant-api03-first"))
        .unwrap()
        .account;
    // An `add` over x with an OAuth login wrote the vault and died before its metadata landed.
    let cred = Fx::credential_json("me@work.co", "rt-1")
        .to_string()
        .into_bytes();
    fx.kc.put("tagteam", x.id.as_str(), &cred);
    let identity = fx
        .cc
        .parse_identity(&Fx::oauth_account("me@work.co"))
        .unwrap();
    let meta = tagteam_engine::store::LoginMeta {
        identity_key: "me@work.co\n",
        identity: &identity,
        kind: "oauth",
        login_expires_at: None,
    };
    fx.engine
        .store()
        .unwrap()
        .begin_replacement(&x.id, fx.cc.fingerprint(&cred).unwrap().as_str(), &meta)
        .unwrap();
    assert!(
        matches!(fx.engine.add_token(token("sk-ant-api03-second")), Err(EngineError::InvalidInput(m)) if m.contains("oauth"))
    );
    assert_eq!(
        fx.vault_refresh_token(&x.id).as_deref(),
        Some("rt-1"),
        "the recovered OAuth credential stays"
    );
}

#[test]
fn a_landed_replacement_can_make_add_token_valid() {
    let fx = Fx::new();
    let x = fx.add("me@work.co", "rt-1"); // an OAuth account
    // An `add` over x with a setup-token login wrote the vault and died before its metadata.
    let setup =
        br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-old","scopes":["user:inference"]}}"#;
    fx.kc.put("tagteam", x.as_str(), setup);
    let identity = fx
        .cc
        .parse_identity(&Fx::oauth_account("me@work.co"))
        .unwrap();
    let meta = tagteam_engine::store::LoginMeta {
        identity_key: "me@work.co\n",
        identity: &identity,
        kind: "setup_token",
        login_expires_at: None,
    };
    fx.engine
        .store()
        .unwrap()
        .begin_replacement(&x, fx.cc.fingerprint(setup).unwrap().as_str(), &meta)
        .unwrap();
    let token = AddTokenOptions {
        email: Some("me@work.co".into()),
        ..token_opts(&fx, "sk-ant-oat01-new")
    };
    let out = fx.engine.add_token(token).unwrap();
    assert_eq!(
        (out.account.id, out.account.kind.as_str()),
        (x, "setup_token")
    );
}

#[test]
fn an_out_of_range_position_creates_nothing() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    assert!(
        fx.engine
            .add_live(AddOptions {
                position: Some(100),
                ..add_opts(&fx)
            })
            .is_err()
    );
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn invalid_token_input_creates_nothing() {
    let fx = Fx::new();
    let bad_email = AddTokenOptions {
        email: Some("nope".into()),
        ..token_opts(&fx, "sk-ant-api03-x")
    };
    assert!(fx.engine.add_token(bad_email).is_err());
    let zero = AddTokenOptions {
        position: Some(0),
        ..token_opts(&fx, "sk-ant-api03-x")
    };
    assert!(fx.engine.add_token(zero).is_err());
    assert!(
        !fx.env.data_dir().exists(),
        "a command that changes nothing creates nothing"
    );
}

#[test]
fn add_token_stores_api_keys_and_setup_tokens() {
    let fx = Fx::new();
    let k = fx
        .engine
        .add_token(token_opts(&fx, "  sk-ant-api03-abc\n"))
        .unwrap()
        .account;
    assert_eq!(
        (k.kind.as_str(), k.label.as_str()),
        ("api_key", "api-key-1@token.local")
    );
    assert_eq!(fx.vault_bytes(&k.id).unwrap(), b"sk-ant-api03-abc");
    let s = fx
        .engine
        .add_token(token_opts(&fx, "sk-ant-oat01-setup"))
        .unwrap()
        .account;
    assert_eq!(
        (s.kind.as_str(), s.label.as_str()),
        ("setup_token", "setup-token-2@token.local")
    );
    let v: serde_json::Value = serde_json::from_slice(&fx.vault_bytes(&s.id).unwrap()).unwrap();
    assert_eq!(
        v,
        json!({"claudeAiOauth": {"accessToken": "sk-ant-oat01-setup", "scopes": ["user:inference"]}})
    );
    assert_eq!(
        fx.engine.store().unwrap().active(&fx.provider()).unwrap(),
        None
    );
}

#[test]
fn add_token_validates_its_inputs() {
    let fx = Fx::new();
    assert!(matches!(
        fx.engine.add_token(token_opts(&fx, "  ")),
        Err(EngineError::InvalidInput(_))
    ));
    let bad_email = AddTokenOptions {
        email: Some("not-an-email".into()),
        ..token_opts(&fx, "sk-ant-api03-x")
    };
    assert!(matches!(
        fx.engine.add_token(bad_email),
        Err(EngineError::InvalidInput(_))
    ));
    let email = Some("shared@x.co".to_string());
    fx.engine
        .add_token(AddTokenOptions {
            email: email.clone(),
            ..token_opts(&fx, "sk-ant-api03-x")
        })
        .unwrap();
    let clash = AddTokenOptions {
        email,
        ..token_opts(&fx, "sk-ant-oat01-y")
    };
    assert!(
        matches!(fx.engine.add_token(clash), Err(EngineError::InvalidInput(m)) if m.contains("api_key"))
    );
    let bad_alias = AddTokenOptions {
        alias: Some("123".into()),
        ..token_opts(&fx, "sk-ant-api03-z")
    };
    assert!(matches!(
        fx.engine.add_token(bad_alias),
        Err(EngineError::InvalidInput(_))
    ));
    let zero = AddTokenOptions {
        position: Some(0),
        ..token_opts(&fx, "sk-ant-api03-z")
    };
    assert!(matches!(
        fx.engine.add_token(zero),
        Err(EngineError::InvalidInput(_))
    ));
}

#[test]
fn account_commands_refuse_inside_a_run_shell() {
    let fx = Fx::new();
    fx.login("me@work.co", "rt-1");
    let mut env = fx.env.clone();
    env.claude_config_dir = Some(fx.env.data_dir().join("sessions/x").into_os_string());
    let engine = fx.engine_with_env(env);
    assert!(matches!(
        engine.add_live(add_opts(&fx)),
        Err(EngineError::InsideRunShell)
    ));
    assert!(matches!(
        engine.add_token(token_opts(&fx, "sk-ant-api03-x")),
        Err(EngineError::InsideRunShell)
    ));
}
