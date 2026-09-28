mod common;

use common::Fx;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::AddTokenOptions;
use tagteam_engine::store::NewAccount;
use tagteam_engine::views::StatusView;
use tagteam_provider::Provider;

#[test]
fn references_resolve_by_position_alias_and_email() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.engine.set_alias(&b, Some("Home")).unwrap();
    assert_eq!(fx.engine.resolve("1", None).unwrap().id, a);
    assert_eq!(fx.engine.resolve("HOME", None).unwrap().id, b);
    assert_eq!(fx.engine.resolve("a@x.co", None).unwrap().id, a);
    assert!(matches!(
        fx.engine.resolve("9", None),
        Err(EngineError::NoSuchAccount(_))
    ));
    assert!(matches!(
        fx.engine.resolve("", None),
        Err(EngineError::NoSuchAccount(_))
    ));
}

#[test]
fn an_email_in_several_providers_is_ambiguous_unless_narrowed() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let other = ProviderId::new("fake-agent");
    let identity = fx.cc.token_identity("a@x.co");
    fx.engine
        .store()
        .unwrap()
        .insert_account(&NewAccount {
            id: &AccountId::from_string("fake-1"),
            provider: &other,
            position: 1,
            identity_key: "a@x.co\n",
            identity: &identity,
            kind: "api_key",
            alias: None,
            login_expires_at: None,
            added_at: 0,
        })
        .unwrap();
    assert!(
        matches!(fx.engine.resolve("a@x.co", None), Err(EngineError::Ambiguous { candidates, .. }) if candidates.len() == 2)
    );
    assert_eq!(fx.engine.candidates("a@x.co", None).unwrap().len(), 2);
    assert_eq!(
        fx.engine
            .resolve("a@x.co", Some(&other))
            .unwrap()
            .id
            .as_str(),
        "fake-1"
    );
}

#[test]
fn remove_deletes_vault_and_row_but_never_the_live_login() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.engine.remove(&a).unwrap();
    assert!(fx.vault_bytes(&a).is_none());
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a"));
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}

#[test]
fn alias_disable_and_move() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.engine.set_alias(&a, Some("work")).unwrap();
    assert!(matches!(
        fx.engine.set_alias(&b, Some("Work")),
        Err(EngineError::InvalidInput(_))
    ));
    assert!(matches!(
        fx.engine.set_alias(&b, Some("12")),
        Err(EngineError::InvalidInput(_))
    ));
    assert_eq!(fx.engine.set_alias(&a, None).unwrap().alias, None);
    assert!(fx.engine.set_disabled(&a, true).unwrap().disabled);
    assert!(!fx.engine.set_disabled(&a, false).unwrap().disabled);
    assert_eq!(fx.engine.move_to(&a, 2).unwrap().position, 2);
    assert_eq!(
        fx.engine
            .store()
            .unwrap()
            .account(&b)
            .unwrap()
            .unwrap()
            .position,
        1
    );
    assert!(matches!(
        fx.engine.move_to(&a, 100),
        Err(EngineError::InvalidInput(_))
    ));
    assert!(matches!(
        fx.engine.move_to(&a, 0),
        Err(EngineError::InvalidInput(_))
    ));
}

#[test]
fn views_follow_the_live_identity() {
    let fx = Fx::new();
    assert!(matches!(
        fx.engine.status(&fx.provider()).unwrap(),
        StatusView::NoLogin
    ));
    let lists = fx.engine.accounts(None).unwrap();
    assert_eq!(lists.len(), 1);
    assert!(lists[0].accounts.is_empty());

    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.engine
        .add_token(AddTokenOptions {
            provider: fx.provider(),
            token: "sk-ant-api03-k".into(),
            position: None,
            email: None,
            alias: None,
            yes: false,
        })
        .unwrap();
    let list = &fx.engine.accounts(None).unwrap()[0];
    assert_eq!(list.active_position, Some(2));
    assert_eq!(list.accounts.iter().filter(|v| v.active).count(), 1);

    fx.login("a@x.co", "rt-a2"); // CC logged in as `a` directly
    match fx.engine.status(&fx.provider()).unwrap() {
        StatusView::Managed { account, total } => assert_eq!((account.row.id, total), (a, 3)),
        _ => panic!("expected a managed status"),
    }
    fx.login("stranger@x.co", "rt-s");
    assert!(
        matches!(fx.engine.status(&fx.provider()).unwrap(), StatusView::Unmanaged { email } if email == "stranger@x.co")
    );
}
