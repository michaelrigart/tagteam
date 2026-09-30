mod common;

use common::Fx;
use serde_json::json;
use tagteam_core::{AccountId, OracleVerdict};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::oracle::verdict;
use tagteam_engine::store::{LoginMeta, NewAccount};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::{Identity, Provider};

#[test]
fn reading_commands_never_create_the_store() {
    let fx = Fx::new();
    assert!(fx.engine.existing_store().unwrap().is_none());
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn account_locks_are_taken_in_ascending_order_without_duplicates() {
    let fx = Fx::new();
    let (a, b) = (AccountId::from_string("b-2"), AccountId::from_string("a-1"));
    let locks = fx.engine.lock_accounts(&[&a, &b, &a]).unwrap();
    assert_eq!(
        locks.iter().map(|l| l.id().as_str()).collect::<Vec<_>>(),
        vec!["a-1", "b-2"]
    );
    assert!(AccountLock::try_acquire(&fx.env, &a).unwrap().is_none());
    drop(locks);
    assert!(AccountLock::try_acquire(&fx.env, &a).unwrap().is_some());
}

#[test]
fn a_pending_replacement_is_reconciled_by_the_next_lock_holder() {
    let fx = Fx::new();
    let store = fx.engine.store().unwrap();
    let id = AccountId::from_string("acc");
    let identity = fx.cc.token_identity("t@token.local");
    store
        .insert_account(&NewAccount {
            id: &id,
            provider: &fx.provider(),
            position: 1,
            identity_key: "t@token.local\n",
            identity: &identity,
            kind: "api_key",
            alias: None,
            login_expires_at: None,
            added_at: 1,
        })
        .unwrap();
    // The replacement installs an OAuth login with new metadata.
    let oauth = fx
        .cc
        .parse_identity(&Fx::oauth_account("t@token.local"))
        .unwrap();
    let meta = LoginMeta {
        identity_key: "t@token.local\n",
        identity: &oauth,
        kind: "oauth",
        login_expires_at: Some(7),
    };
    // A replacement that died before writing the vault: rolled back, metadata untouched.
    store
        .begin_replacement(&id, "sha256:never-written", &meta)
        .unwrap();
    drop(fx.engine.lock_account(&id).unwrap());
    let row = store.account(&id).unwrap().unwrap();
    assert_eq!(
        (row.login_epoch, row.replacing_fp, row.kind.as_str()),
        (0, None, "api_key")
    );
    // One that wrote the vault and then died: finished, with its kind and metadata.
    let cred = Fx::credential_json("t@token.local", "rt-new")
        .to_string()
        .into_bytes();
    fx.kc.put(SERVICE, id.as_str(), &cred);
    let fp = fx.cc.fingerprint(&cred).unwrap();
    store.begin_replacement(&id, fp.as_str(), &meta).unwrap();
    drop(fx.engine.lock_account(&id).unwrap());
    let row = store.account(&id).unwrap().unwrap();
    assert_eq!(
        (row.login_epoch, row.replacing_fp, row.kind.as_str()),
        (1, None, "oauth")
    );
    assert_eq!(
        (row.account_uuid.as_deref(), row.login_expires_at),
        (Some("uuid-t@token.local"), Some(7))
    );
}

#[test]
fn oracle_verdicts_need_a_positive_uuid_match() {
    let fx = Fx::new();
    let row_for = |uuid: Option<&str>| tagteam_engine::store::AccountRow {
        id: AccountId::from_string("a"),
        provider: fx.provider(),
        position: 1,
        identity_key: "a@b.co\n".into(),
        label: "a@b.co".into(),
        email: Some("a@b.co".into()),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: uuid.map(str::to_owned),
        kind: "oauth".into(),
        alias: None,
        disabled: false,
        identity_json: json!({}),
        login_expires_at: None,
        login_epoch: 0,
        replacing_fp: None,
        quarantine_reason: None,
        quarantine_fp: None,
        quarantine_at: None,
        added_at: 0,
    };
    let id = |email: &str, uuid: &str| Identity {
        label: email.into(),
        email: Some(email.into()),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: Some(uuid.into()),
        raw: json!({}),
    };
    assert_eq!(
        verdict(None, &row_for(Some("u"))),
        OracleVerdict::Unavailable
    );
    assert_eq!(
        verdict(Some(&id("a@b.co", "u")), &row_for(Some("u"))),
        OracleVerdict::ThisAccount
    );
    // Same email, conflicting uuid: a recycled email is another account.
    assert_eq!(
        verdict(Some(&id("a@b.co", "v")), &row_for(Some("u"))),
        OracleVerdict::OtherIdentity
    );
    // No stored uuid yet: email and org must agree.
    assert_eq!(
        verdict(Some(&id("a@b.co", "v")), &row_for(None)),
        OracleVerdict::ThisAccount
    );
    assert_eq!(
        verdict(Some(&id("z@b.co", "v")), &row_for(None)),
        OracleVerdict::OtherIdentity
    );
    // A resolved identity with no uuid of its own carries no attribution signal, whether or
    // not the stored account already has one, and regardless of how well the email matches.
    let no_uuid = Identity {
        label: "a@b.co".into(),
        email: Some("a@b.co".into()),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: None,
        raw: json!({}),
    };
    let empty_uuid = Identity {
        account_uuid: Some(String::new()),
        ..no_uuid.clone()
    };
    assert_eq!(
        verdict(Some(&no_uuid), &row_for(Some("u"))),
        OracleVerdict::Unavailable
    );
    assert_eq!(
        verdict(Some(&empty_uuid), &row_for(Some("u"))),
        OracleVerdict::Unavailable
    );
    assert_eq!(
        verdict(Some(&no_uuid), &row_for(None)),
        OracleVerdict::Unavailable
    );
}

#[test]
fn views_carry_each_rows_kind_traits_from_its_provider() {
    let fx = Fx::new();
    fx.add("a@b.co", "rt-a");
    fx.add_api_key(common::API_KEY);
    let lists = fx.engine.accounts(None).unwrap();
    let kinds: Vec<_> = lists[0]
        .accounts
        .iter()
        .map(|v| (v.row.kind.as_str(), v.kind))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("oauth", fx.cc.kind_traits("oauth")),
            ("api_key", fx.cc.kind_traits("api_key")),
        ]
    );
    let key_row = lists[0].accounts[1].row.clone();
    let view = fx.engine.account_view(key_row, false);
    assert!(view.kind.managed_key_axis);
    assert_eq!(view.kind.display, Some("api key"));
}
