use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{EventRow, JournalRow, LoginMeta, NewAccount, Store, StoreError};
use tagteam_provider::{Identity, ProcessStamp};

fn identity(email: &str) -> Identity {
    Identity {
        label: email.into(),
        email: Some(email.into()),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: None,
        raw: json!({"emailAddress": email}),
    }
}

fn add(s: &Store, p: &ProviderId, id: &str, email: &str, pos: u32) -> AccountId {
    let aid = AccountId::from_string(id);
    let key = format!("{email}\n");
    s.insert_account(&NewAccount {
        id: &aid,
        provider: p,
        position: pos,
        identity_key: &key,
        identity: &identity(email),
        kind: "oauth",
        alias: None,
        login_expires_at: None,
        added_at: 1,
    })
    .unwrap();
    aid
}

fn cc() -> ProviderId {
    ProviderId::new("claude-code")
}

/// Reads the on-disk journal mode through a fresh, independent connection, so the assertion
/// reflects what was actually persisted rather than one `Store`'s in-memory view of it.
fn journal_mode(path: &std::path::Path) -> String {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn opening_migrates_once_and_open_existing_never_creates() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("data/tagteam/tagteam.db");
    assert!(Store::open_existing(&path).unwrap().is_none());
    assert!(!path.parent().unwrap().exists());
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 1);
    drop(s);
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 1);
    assert!(Store::open_existing(&path).unwrap().is_some());
}

#[test]
fn positions_are_per_provider_and_gaps_are_not_reused() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let other = ProviderId::new("fake-agent");
    add(&s, &cc(), "a", "a@x.co", 1);
    add(&s, &cc(), "b", "b@x.co", 4);
    add(&s, &other, "c", "a@x.co", 1); // same email, other provider: another account
    assert_eq!(s.next_position(&cc()).unwrap(), 5);
    assert_eq!(s.next_position(&other).unwrap(), 2);
    assert_eq!(s.next_position(&ProviderId::new("none")).unwrap(), 1);
    let rows = s.accounts(&cc()).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.position).collect::<Vec<_>>(),
        vec![1, 4]
    );
    assert_eq!(rows[0].identity_json, json!({"emailAddress": "a@x.co"}));
}

#[test]
fn uniqueness_is_reported_by_name() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let dup = NewAccount {
        id: &AccountId::from_string("z"),
        provider: &cc(),
        position: 1,
        identity_key: "z@x.co\n",
        identity: &identity("z@x.co"),
        kind: "oauth",
        alias: None,
        login_expires_at: None,
        added_at: 1,
    };
    assert!(matches!(
        s.insert_account(&dup),
        Err(StoreError::PositionTaken(1))
    ));
    s.set_alias(&a, Some("work")).unwrap();
    assert!(matches!(
        s.set_alias(&b, Some("WORK")),
        Err(StoreError::AliasTaken(_))
    ));
    assert_eq!(s.find_by_alias("Work").unwrap().unwrap().id, a);
    s.set_alias(&a, None).unwrap();
    assert!(s.find_by_alias("work").unwrap().is_none());
}

#[test]
fn move_swaps_when_the_position_is_taken() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    s.move_to(&a, 2).unwrap();
    assert_eq!(s.account(&a).unwrap().unwrap().position, 2);
    assert_eq!(s.account(&b).unwrap().unwrap().position, 1);
    s.move_to(&a, 7).unwrap();
    assert_eq!(s.account(&a).unwrap().unwrap().position, 7);
}

#[test]
fn replacement_markers_move_the_epoch() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let replacement = Identity {
        account_uuid: Some("u-new".into()),
        ..identity("a@x.co")
    };
    let meta = |kind| LoginMeta {
        identity_key: "a@x.co\n",
        identity: &replacement,
        kind,
        login_expires_at: Some(42),
    };
    s.begin_replacement(&a, "sha256:x", &meta("api_key"))
        .unwrap();
    let r = s.account(&a).unwrap().unwrap();
    assert_eq!(
        (r.login_epoch, r.replacing_fp.as_deref(), r.kind.as_str()),
        (1, Some("sha256:x"), "oauth")
    );
    // Finishing installs the recorded metadata, kind included.
    s.finish_replacement(&a).unwrap();
    let r = s.account(&a).unwrap().unwrap();
    assert_eq!(
        (r.login_epoch, r.replacing_fp, r.kind.as_str()),
        (1, None, "api_key")
    );
    assert_eq!(
        (r.account_uuid.as_deref(), r.login_expires_at),
        (Some("u-new"), Some(42))
    );
    s.begin_replacement(&a, "sha256:y", &meta("setup_token"))
        .unwrap();
    s.rollback_replacement(&a).unwrap();
    let r = s.account(&a).unwrap().unwrap();
    assert_eq!(
        (r.login_epoch, r.replacing_fp, r.kind.as_str()),
        (1, None, "api_key")
    );
}

#[test]
fn commit_switch_is_one_transaction_and_delete_clears_active() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let j = JournalRow {
        provider: cc(),
        holder: ProcessStamp { pid: 1, start: 2 },
        from_id: Some(a.clone()),
        to_id: b.clone(),
        from_fp: Some("sha256:a".into()),
        from_identity: Some(json!({"emailAddress": "a@x.co"})),
        to_fp: "sha256:b".into(),
        started_at: 5,
        prior: None,
    };
    s.insert_journal(&j).unwrap();
    assert_eq!(s.journal(&cc()).unwrap().unwrap(), j);
    // A second write replaces the row in one statement, carrying the row it superseded.
    let replaced = JournalRow {
        started_at: 9,
        prior: Some(Box::new(j.clone())),
        ..j.clone()
    };
    s.insert_journal(&replaced).unwrap();
    assert_eq!(s.journal(&cc()).unwrap().unwrap(), replaced);
    s.insert_journal(&j).unwrap();
    let ev = EventRow {
        at: 6,
        provider: cc(),
        kind: "switch".into(),
        from_id: Some(a.clone()),
        to_id: Some(b.clone()),
        trigger: Some("manual".into()),
        source: "cli".into(),
        detail: None,
    };
    s.commit_switch(&cc(), &b, &ev).unwrap();
    assert_eq!(s.active(&cc()).unwrap(), Some(b.clone()));
    assert!(s.journal(&cc()).unwrap().is_none());
    assert_eq!(s.events().unwrap(), vec![ev]);
    s.delete_account(&b).unwrap();
    assert_eq!(s.active(&cc()).unwrap(), None);
}

#[test]
fn uuid_backfill_only_fills_null() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.backfill_account_uuid(&a, "u-1").unwrap();
    s.backfill_account_uuid(&a, "u-2").unwrap();
    assert_eq!(
        s.account(&a).unwrap().unwrap().account_uuid.as_deref(),
        Some("u-1")
    );
}

#[test]
fn find_by_email_spans_providers_unless_narrowed() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    add(&s, &cc(), "a", "a@x.co", 1);
    add(&s, &ProviderId::new("fake-agent"), "b", "a@x.co", 1);
    assert_eq!(s.find_by_email("a@x.co", None).unwrap().len(), 2);
    assert_eq!(s.find_by_email("a@x.co", Some(&cc())).unwrap().len(), 1);
}

// --- Fix round 1 ---

#[test]
fn a_newer_schema_version_is_refused_and_the_database_is_untouched() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    {
        // A file as a hypothetical newer tagteam would leave it: schema v2, still in
        // whatever journal mode it was created with, never touched by this build's
        // `connect`/`migrate`. Using a raw connection (rather than `Store::open` followed by
        // bumping `user_version`) means the file starts in the default `delete` journal mode,
        // so switching it to WAL is a real, detectable mutation, not a no-op.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE accounts (id TEXT PRIMARY KEY); PRAGMA user_version = 2;")
            .unwrap();
    }
    assert!(matches!(
        Store::open(&path),
        Err(StoreError::UnsupportedSchema(2))
    ));
    // Refusing must not have touched the database at all: same version, same journal mode
    // (never switched to WAL), same table set (no re-migration, no half-applied schema).
    let conn = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);
    assert_eq!(journal_mode(&path), "delete");
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(tables, vec!["accounts".to_string()]);
}

#[test]
fn every_open_path_leaves_the_store_in_wal_mode() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    // A fresh file migrates from the default journal mode and is switched to WAL right after.
    let s = Store::open(&path).unwrap();
    assert_eq!(journal_mode(&path), "wal");
    drop(s);
    // Reopening an already-migrated (v1) file still calls through to set WAL; a no-op there.
    let s = Store::open(&path).unwrap();
    assert_eq!(journal_mode(&path), "wal");
    drop(s);
    let s = Store::open_existing(&path).unwrap().unwrap();
    assert_eq!(journal_mode(&path), "wal");
    drop(s);
}

#[test]
fn open_existing_reports_an_unreadable_store_instead_of_absent() {
    // Root ignores permission bits, so chmod 0000 would not actually make the file unreadable
    // and the assertion below would fail for a reason unrelated to what this test checks.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }

    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    Store::open(&path).unwrap();

    // Restores the mode even if an assertion below panics, so the tempdir can still clean
    // itself up on drop.
    struct RestoreMode {
        path: std::path::PathBuf,
        mode: u32,
    }
    impl Drop for RestoreMode {
        fn drop(&mut self) {
            let _ =
                std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(self.mode));
        }
    }
    let original_mode = std::fs::metadata(&path).unwrap().permissions().mode();
    let _restore = RestoreMode {
        path: path.clone(),
        mode: original_mode,
    };

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(Store::open_existing(&path).is_err());
}

#[test]
fn concurrent_first_opens_all_succeed() {
    for _ in 0..5 {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("t.db");
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || Store::open(&path).and_then(|s| s.schema_version()))
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap().unwrap(), 1);
        }
    }
}

#[test]
fn finish_replacement_refuses_metadata_missing_required_fields() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let replacement = identity("a@x.co");
    let meta = LoginMeta {
        identity_key: "a@x.co\n",
        identity: &replacement,
        kind: "api_key",
        login_expires_at: Some(42),
    };
    s.begin_replacement(&a, "sha256:x", &meta).unwrap();
    drop(s);
    {
        // Corrupt the recorded metadata as if written by an incompatible version: no
        // identity_key, label or kind at all.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE accounts SET replacing_meta = ?1 WHERE id = ?2",
            rusqlite::params![r#"{"org_uuid":""}"#, "a"],
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert!(matches!(
        s.finish_replacement(&a),
        Err(StoreError::Corrupt(_))
    ));
    // Untouched: the original login and the pending marker are both still there.
    let r = s.account(&a).unwrap().unwrap();
    assert_eq!(r.kind, "oauth");
    assert_eq!(r.replacing_fp.as_deref(), Some("sha256:x"));
}

#[test]
fn begin_replacement_is_guarded_against_a_second_start() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let replacement = identity("a@x.co");
    let meta = |kind| LoginMeta {
        identity_key: "a@x.co\n",
        identity: &replacement,
        kind,
        login_expires_at: None,
    };
    s.begin_replacement(&a, "sha256:x", &meta("api_key"))
        .unwrap();
    assert!(matches!(
        s.begin_replacement(&a, "sha256:y", &meta("setup_token")),
        Err(StoreError::ReplacementPending)
    ));
}

#[test]
fn rollback_after_rollback_leaves_the_epoch_at_its_start() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let start = s.account(&a).unwrap().unwrap().login_epoch;
    let replacement = identity("a@x.co");
    let meta = LoginMeta {
        identity_key: "a@x.co\n",
        identity: &replacement,
        kind: "oauth",
        login_expires_at: None,
    };
    s.begin_replacement(&a, "sha256:x", &meta).unwrap();
    s.rollback_replacement(&a).unwrap();
    s.rollback_replacement(&a).unwrap();
    assert_eq!(s.account(&a).unwrap().unwrap().login_epoch, start);
}

#[test]
fn a_missing_account_is_reported_by_both_replacement_entry_points() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let missing = AccountId::from_string("ghost");
    let replacement = identity("a@x.co");
    let meta = LoginMeta {
        identity_key: "a@x.co\n",
        identity: &replacement,
        kind: "oauth",
        login_expires_at: None,
    };
    assert!(matches!(
        s.begin_replacement(&missing, "sha256:x", &meta),
        Err(StoreError::NoSuchAccount)
    ));
    assert!(matches!(
        s.finish_replacement(&missing),
        Err(StoreError::NoSuchAccount)
    ));
}

#[test]
fn a_malformed_prior_journal_snapshot_is_reported_not_silently_dropped() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let j = JournalRow {
        provider: cc(),
        holder: ProcessStamp { pid: 1, start: 2 },
        from_id: Some(a),
        to_id: b,
        from_fp: Some("sha256:a".into()),
        from_identity: None,
        to_fp: "sha256:b".into(),
        started_at: 5,
        prior: None,
    };
    s.insert_journal(&j).unwrap();
    drop(s);
    {
        // A `prior` snapshot missing a required field: valid JSON, not a valid JournalRow.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE switch_journal SET prior = ?1 WHERE provider = ?2",
            rusqlite::params![r#"{"provider":"claude-code"}"#, "claude-code"],
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert!(s.journal(&cc()).is_err());
}

#[test]
fn next_position_reports_overflow_instead_of_wrapping() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    add(&s, &cc(), "a", "a@x.co", u32::MAX);
    assert!(s.next_position(&cc()).is_err());
}

#[test]
fn empty_aliases_are_refused_and_never_match() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    assert!(matches!(
        s.set_alias(&a, Some("")),
        Err(StoreError::InvalidAlias)
    ));
    assert!(s.find_by_alias("").unwrap().is_none());
}

#[test]
fn open_existing_does_not_recreate_a_removed_database() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    Store::open(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(Store::open_existing(&path).unwrap().is_none());
    assert!(!path.exists());
}
