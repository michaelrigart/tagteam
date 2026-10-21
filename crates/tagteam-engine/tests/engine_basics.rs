mod common;

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use common::Fx;
use serde_json::json;
use tagteam_core::{AccountId, OracleVerdict, ProviderId};
use tagteam_engine::account_lock::AccountLock;
use tagteam_engine::oracle::verdict;
use tagteam_engine::store::{LoginMeta, NewAccount, Store};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::process::{Captured, SpawnSpec};
use tagteam_provider::{Cancel, Identity, Provider};

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
        from_live: false,
    };
    // A replacement that died before writing the vault: rolled back, metadata untouched.
    store
        .begin_replacement(&id, "sha256:never-written", &meta, false)
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
    store
        .begin_replacement(&id, fp.as_str(), &meta, false)
        .unwrap();
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

#[test]
fn data_version_is_none_without_a_store_and_moves_only_for_another_connection() {
    // D3 change 4: the daemon's own writes never trigger the store watch (D4.4).
    let fx = Fx::new();
    assert_eq!(fx.engine.data_version().unwrap(), None);
    assert_eq!(fx.engine.store_file(), fx.env.data_dir().join("tagteam.db"));
    assert!(!fx.engine.store_file().exists(), "never creates the store");
    fx.add("a@x.co", "rt-a");
    let first = fx.engine.data_version().unwrap().unwrap();
    fx.engine
        .store()
        .unwrap()
        .set_unhealthy_ticks(&fx.provider(), 1)
        .unwrap();
    assert_eq!(
        fx.engine.data_version().unwrap(),
        Some(first),
        "its own write"
    );
    Store::open(&fx.engine.store_file())
        .unwrap()
        .set_unhealthy_ticks(&fx.provider(), 2)
        .unwrap();
    let second = fx.engine.data_version().unwrap().unwrap();
    assert_ne!(second, first, "another connection committed");
}

#[test]
fn forget_store_makes_the_next_use_open_the_file_again() {
    // Decision 11: a store deleted and created again under a running engine (its data
    // directory removed, then a first `add`) is read from the new file, never from the
    // unlinked one the cached connection still holds.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let file = fx.engine.store_file();
    let remove_all = || {
        for name in ["tagteam.db", "tagteam.db-wal", "tagteam.db-shm"] {
            let _ = fs::remove_file(fx.env.data_dir().join(name));
        }
    };
    let before = fx.engine.existing_store().unwrap().unwrap();
    remove_all();
    let replacement = Store::open(&file).unwrap();
    common::add(&replacement, &fx.provider(), "other", "z@x.co", 1);
    drop(replacement);
    assert!(
        Arc::ptr_eq(&fx.engine.existing_store().unwrap().unwrap(), &before),
        "still the cached connection"
    );
    drop(before);
    fx.engine.forget_store();
    let ids: Vec<AccountId> = fx
        .engine
        .existing_store()
        .unwrap()
        .unwrap()
        .accounts(&fx.provider())
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, [AccountId::from_string("other")]);
    // A file that is gone is no store, and nothing creates it.
    fx.engine.forget_store();
    remove_all();
    assert!(fx.engine.existing_store().unwrap().is_none());
    assert!(!file.exists());
}

#[test]
fn the_spawner_is_the_port_the_engine_was_built_with() {
    // D6.2: the notifier delivers through the engine's own port, so tests script it.
    let fx = Fx::new();
    fx.spawner.push(Captured::Exited {
        code: Some(0),
        signal: None,
        stdout: Vec::new(),
        stderr: Vec::new(),
    });
    let spec = SpawnSpec {
        program: "/usr/bin/osascript".into(),
        ..SpawnSpec::default()
    };
    let ran = fx
        .engine
        .spawner()
        .run_captured(&spec, Duration::from_secs(5), &Cancel::new());
    assert!(
        matches!(ran, Captured::Exited { code: Some(0), .. }),
        "{ran:?}"
    );
    assert_eq!(fx.spawner.specs(), [spec]);
}

#[test]
fn switchable_counts_read_the_store_alone_and_count_a_quarantine() {
    let fx = Fx::new();
    assert_eq!(fx.engine.switchable_count(&common::cc()).unwrap(), 0);
    assert_eq!(fx.engine.switchable_providers().unwrap(), []);
    assert!(!fx.engine.store_file().exists());
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.quarantine(&a, "invalid_grant", "sha256:x");
    assert_eq!(
        fx.engine.switchable_count(&common::cc()).unwrap(),
        2,
        "a quarantine still counts"
    );
    let newer = ProviderId::new("newer-agent");
    let store = fx.engine.store().unwrap();
    common::add(&store, &newer, "n-1", "n1@x.co", 1);
    common::add(&store, &newer, "n-2", "n2@x.co", 2);
    assert_eq!(
        fx.engine.switchable_providers().unwrap(),
        [common::cc(), newer.clone()]
    );
    fx.engine.set_disabled(&b, true).unwrap();
    assert_eq!(fx.engine.switchable_count(&common::cc()).unwrap(), 1);
    assert_eq!(fx.engine.switchable_providers().unwrap(), [newer]);
}
