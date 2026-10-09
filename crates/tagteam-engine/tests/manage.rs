mod common;

use std::fs;

use common::{Fx, block_rescue, credential, unblock_rescue, vault_fp};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::lifecycle::AddTokenOptions;
use tagteam_engine::store::{JournalRow, NewAccount};
use tagteam_engine::vault::SERVICE;
use tagteam_engine::views::StatusView;
use tagteam_provider::profile::ProfileMarker;
use tagteam_provider::{ProcessStamp, Provider, Read};

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
    // A second capture of the same account creates a `.prev` generation (§6.2), which
    // `remove` must also delete.
    fx.login("a@x.co", "rt-a2");
    fx.engine.add_live(fx.add_options()).unwrap();
    assert!(fx.kc.get(SERVICE, &format!("{a}.prev")).is_some());

    fx.engine.remove(&a).unwrap();
    assert!(fx.vault_bytes(&a).is_none());
    assert!(fx.kc.get(SERVICE, &format!("{a}.prev")).is_none());
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"));
    assert_eq!(fx.live_email().as_deref(), Some("a@x.co"));
}

#[test]
fn management_commands_on_a_missing_account_create_no_store() {
    let fx = Fx::new();
    let id = AccountId::from_string("nope");

    assert!(matches!(
        fx.engine.remove(&id),
        Err(EngineError::NoSuchAccount(_))
    ));
    assert!(!fx.env.data_dir().exists());

    assert!(matches!(
        fx.engine.set_alias(&id, Some("x")),
        Err(EngineError::NoSuchAccount(_))
    ));
    assert!(!fx.env.data_dir().exists());

    assert!(matches!(
        fx.engine.set_disabled(&id, true),
        Err(EngineError::NoSuchAccount(_))
    ));
    assert!(!fx.env.data_dir().exists());
    assert!(matches!(
        fx.engine.set_disabled(&id, false),
        Err(EngineError::NoSuchAccount(_))
    ));
    assert!(!fx.env.data_dir().exists());

    assert!(matches!(
        fx.engine.move_to(&id, 1),
        Err(EngineError::NoSuchAccount(_))
    ));
    assert!(!fx.env.data_dir().exists());
}

/// §9.6, amended: an interrupted switch refuses `remove` (account-changing) but never the
/// metadata-only commands.
#[test]
fn metadata_commands_proceed_through_an_interrupted_switch_but_remove_refuses() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    // Undecidable (§9.6): no live secret is `to_fp`, and it names no outgoing account, so the
    // recovery each command runs under the mutation lock keeps it.
    let journal = JournalRow {
        provider: fx.provider(),
        holder: ProcessStamp {
            pid: 999_999,
            start: 0,
        },
        from_id: None,
        to_id: b,
        from_fp: None,
        from_identity: None,
        to_fp: "sha256:stale".into(),
        to_epoch: None,
        started_at: 1,
        prior: None,
    };
    fx.engine.store().unwrap().insert_journal(&journal).unwrap();

    assert!(matches!(
        fx.engine.remove(&a),
        Err(EngineError::InterruptedSwitch(_))
    ));

    fx.engine.set_alias(&a, Some("work")).unwrap();
    assert!(fx.engine.set_disabled(&a, true).unwrap().disabled);
    assert!(!fx.engine.set_disabled(&a, false).unwrap().disabled);
    assert_eq!(fx.engine.move_to(&a, 2).unwrap().position, 2);
    assert_eq!(
        fx.engine.store().unwrap().journal(&fx.provider()).unwrap(),
        Some(journal),
        "they proceeded through the interrupted switch, not after it was settled"
    );
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
    assert!(
        !fx.env.data_dir().exists(),
        "read-only views create nothing on a fresh machine (§5)"
    );

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

#[test]
fn remove_deletes_the_accounts_rescue_files_and_nobody_elses() {
    // A rescue holds a live refresh token (§6.3): it must not outlive its account.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let rescue = |id: &AccountId, rt: &str| {
        fx.plant_rescue(id, &vault_fp(&fx, id), &credential("a@x.co", rt))
    };
    let readable = rescue(&a, "rt-a-2");
    let garbage = fx
        .env
        .data_dir()
        .join("rescue")
        .join(format!("{a}-junk.json"));
    fs::write(&garbage, "not json").unwrap();
    let others = rescue(&b, "rt-b-2");

    fx.engine.remove(&a).unwrap();
    assert!(!readable.exists());
    assert!(
        !garbage.exists(),
        "an unreadable rescue is deleted by name too"
    );
    assert!(others.exists(), "another account's rescue is untouched");
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
    assert!(fx.vault_bytes(&a).is_none());
}

#[test]
fn remove_never_creates_the_rescue_directory() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.engine.remove(&a).unwrap();
    assert!(!fx.env.data_dir().join("rescue").exists());
}

#[test]
fn a_rescue_that_cannot_be_deleted_fails_remove_before_the_row_goes() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let rescue = fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a-2"));
    block_rescue(&fx);
    let result = fx.engine.remove(&a);
    unblock_rescue(&fx);
    assert!(result.is_err(), "the refresh token is still on disk");
    assert!(rescue.exists());
    assert!(
        fx.engine.store().unwrap().account(&a).unwrap().is_some(),
        "the row stays, so the remove can be retried"
    );
    fx.engine.remove(&a).unwrap();
    assert!(!rescue.exists());
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
}

#[test]
fn remove_refuses_a_rescue_path_it_cannot_list_before_deleting_anything() {
    // §6.3: a `rescue` that is not a directory hides every account's rescues; `remove` refuses,
    // naming it, rather than guess which entries were the account's.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let rescue = fx.env.data_dir().join("rescue");
    fs::write(&rescue, "not a directory").unwrap();
    let err = fx.engine.remove(&a).unwrap_err();
    assert!(
        matches!(&err, EngineError::RescueUnlistable { path, .. } if path == &rescue),
        "{err:?}"
    );
    assert_eq!(err.kind(), "rescue-unreadable");
    assert!(
        err.to_string().contains(&rescue.display().to_string()),
        "{err}"
    );
    assert!(fx.vault_bytes(&a).is_some(), "the vault was not touched");
    assert!(rescue.is_file(), "nor the path");
    fs::remove_file(&rescue).unwrap();
    fx.engine.remove(&a).unwrap();
}

/// The live login's items and what each holds.
type LiveItems = [((String, String), &'static [u8]); 2];

/// A macOS fixture whose `CLAUDE_CONFIG_DIR` names a directory holding the live login, with the
/// live login's two items planted; the account `a` is stored beside `b`.
fn live_config_fixture() -> (Fx, AccountId, std::path::PathBuf, LiveItems) {
    let fx = Fx::with(tagteam_cc::live::Platform::MacOs, |e| {
        let parent = fs::canonicalize(e.home.parent().unwrap()).unwrap();
        let live = parent.join(e.home.file_name().unwrap()).join(".claude");
        e.claude_config_dir = Some(live.into_os_string());
    });
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    let live_dir = std::path::PathBuf::from(fx.env.claude_config_dir.clone().unwrap());
    fs::create_dir_all(&live_dir).unwrap();
    fs::write(live_dir.join("settings.json"), b"{}").unwrap();
    let items = [
        (
            fx.live_item(tagteam_cc::ItemKind::OAuth),
            &b"the live login"[..],
        ),
        (
            fx.live_item(tagteam_cc::ItemKind::ManagedKey),
            &b"the live key"[..],
        ),
    ];
    for (item, bytes) in &items {
        fx.kc.put(&item.0, &item.1, bytes);
    }
    (fx, a, live_dir, items)
}

fn assert_live_items_kept(fx: &Fx, items: &LiveItems) {
    for (item, bytes) in items {
        assert_eq!(
            fx.kc.get(&item.0, &item.1).as_deref(),
            Some(*bytes),
            "the live login's item {item:?} is kept"
        );
    }
}

#[test]
fn remove_of_a_profile_that_is_a_link_to_the_live_config_dir_never_deletes_the_live_login() {
    // Codex pre-merge P1 (§10.3, §10.5): the account's own `sessions/<id>` is a link to the
    // directory `CLAUDE_CONFIG_DIR` names, with no marker. No item is named through a link.
    let (fx, a, live_dir, items) = live_config_fixture();
    let link = fx.profile_dir(&a);
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&live_dir, &link).unwrap();

    fx.engine.remove(&a).unwrap();

    assert_live_items_kept(&fx, &items);
    assert!(fs::symlink_metadata(&link).is_err(), "the link is gone");
    assert!(live_dir.join("settings.json").exists(), "the target stays");
    assert!(fx.vault_bytes(&a).is_none());
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_none());
}

#[test]
fn remove_of_a_profile_whose_marker_names_the_live_config_dir_refuses_with_nothing_deleted() {
    let (fx, a, live_dir, items) = live_config_fixture();
    let profile = fx.make_profile(&a);
    let mut marker = match ProfileMarker::read(&profile) {
        Read::Present(m) => m,
        _ => panic!("no marker"),
    };
    marker.config_dir = fx.cc.live_item_spelling(&fx.env).unwrap();
    marker.write(&profile).unwrap();

    let err = fx.engine.remove(&a).unwrap_err();

    assert!(err.to_string().contains("is the live login's"), "{err}");
    assert_live_items_kept(&fx, &items);
    assert!(live_dir.join("settings.json").exists());
    assert!(profile.join(".claude.json").exists(), "the profile is kept");
    assert!(fx.vault_bytes(&a).is_some(), "the vault entry is kept");
    assert!(fx.engine.store().unwrap().account(&a).unwrap().is_some());
}

#[test]
fn remove_of_a_profile_holding_the_live_login_s_files_refuses_with_nothing_deleted() {
    // Codex pre-merge P1b (§10.3, §10.5): the environment names the profile directory in a
    // spelling that differs from its marker's; the live credential file is inside it.
    let id = AccountId::from_string("0192-live-inside");
    let fx = Fx::with(tagteam_cc::live::Platform::Linux, |e| {
        let dir = tagteam_provider::profile::profile_path(e, &id);
        fs::create_dir_all(&dir).unwrap();
        let mut spelled = fs::canonicalize(&dir).unwrap().into_os_string();
        spelled.push("/.");
        e.claude_securestorage_config_dir = Some(spelled);
    });
    let dir = fx.profile_dir(&id);
    fx.write_marker(&dir, &id, &fx.env);
    let live = dir.join(".credentials.json");
    fs::write(&live, b"the live login").unwrap();
    let store = fx.engine.store().unwrap();
    let identity = fx.cc.token_identity("a@x.co");
    store
        .insert_account(&NewAccount {
            id: &id,
            provider: &ProviderId::new("claude-code"),
            position: 1,
            identity_key: "a@x.co\n",
            identity: &identity,
            kind: "oauth",
            alias: None,
            login_expires_at: None,
            added_at: 0,
        })
        .unwrap();
    fx.put_vault(&id, b"a vault credential");

    let err = fx.engine.remove(&id).unwrap_err();

    assert!(
        err.to_string().contains("live login's files are inside"),
        "{err}"
    );
    assert_eq!(fs::read(&live).unwrap(), b"the live login");
    assert!(dir.join(".tagteam-profile.json").exists());
    assert!(fx.vault_bytes(&id).is_some(), "nothing else was deleted");
    assert!(fx.engine.store().unwrap().account(&id).unwrap().is_some());
}

#[test]
fn remove_of_a_profile_a_live_credential_link_leads_into_refuses_with_nothing_deleted() {
    let id = AccountId::from_string("0192-linked-in");
    let fx = Fx::with(tagteam_cc::live::Platform::Linux, |_| {});
    fx.engine
        .store()
        .unwrap()
        .insert_account(&NewAccount {
            id: &id,
            provider: &ProviderId::new("claude-code"),
            position: 1,
            identity_key: "a@x.co\n",
            identity: &fx.cc.token_identity("a@x.co"),
            kind: "oauth",
            alias: None,
            login_expires_at: None,
            added_at: 0,
        })
        .unwrap();
    fx.put_vault(&id, b"a vault credential");
    let dir = fx.profile_dir(&id);
    fx.write_marker(&dir, &id, &fx.env);
    let target = dir.join(".credentials.json");
    fs::write(&target, b"the live login").unwrap();
    let link = fx.paths().credentials_file;
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let err = fx.engine.remove(&id).unwrap_err();

    assert!(
        err.to_string().contains("live login's files are inside"),
        "{err}"
    );
    assert_eq!(fs::read(&target).unwrap(), b"the live login");
    assert!(fs::symlink_metadata(&link).is_ok(), "the link stays");
    assert!(dir.join(".tagteam-profile.json").exists());
    assert!(fx.vault_bytes(&id).is_some(), "nothing else was deleted");
    assert!(fx.engine.store().unwrap().account(&id).unwrap().is_some());
}
