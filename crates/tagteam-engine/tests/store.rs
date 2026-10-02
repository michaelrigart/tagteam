mod common;

use common::{add, cc, identity};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use tagteam_core::autoswitch::{AutoState, Departure, Trigger};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{
    Activation, AutoRecord, EventRow, JournalRow, LoginMeta, NewAccount, Store, StoreError,
};
use tagteam_provider::{Identity, ProcessStamp};

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
    assert_eq!(s.schema_version().unwrap(), 2);
    drop(s);
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 2);
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
        to_epoch: Some(4),
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
    s.commit_switch(&cc(), &b, 0, &ev, None).unwrap();
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
        // A file as a hypothetical newer tagteam would leave it: schema v3, still in
        // whatever journal mode it was created with, never touched by this build's
        // `connect`/`migrate`. Using a raw connection (rather than `Store::open` followed by
        // bumping `user_version`) means the file starts in the default `delete` journal mode,
        // so switching it to WAL is a real, detectable mutation, not a no-op.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE accounts (id TEXT PRIMARY KEY); PRAGMA user_version = 3;")
            .unwrap();
    }
    assert!(matches!(
        Store::open(&path),
        Err(StoreError::UnsupportedSchema(3))
    ));
    // Refusing must not have touched the database at all: same version, same journal mode
    // (never switched to WAL), same table set (no re-migration, no half-applied schema).
    let conn = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
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
    // Reopening an already-migrated (v2) file still calls through to set WAL; a no-op there.
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
            assert_eq!(h.join().unwrap().unwrap(), 2);
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
        to_epoch: None,
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

#[test]
fn quarantines_are_set_bound_to_a_fingerprint_and_cleared() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_quarantine(&a, "invalid_grant", "sha256:sent", 42)
        .unwrap();
    let row = s.account(&a).unwrap().unwrap();
    assert_eq!(
        (
            row.quarantine_reason.as_deref(),
            row.quarantine_fp.as_deref(),
            row.quarantine_at
        ),
        (Some("invalid_grant"), Some("sha256:sent"), Some(42))
    );
    assert!(s.clear_quarantine(&a).unwrap(), "one was set");
    assert!(!s.clear_quarantine(&a).unwrap(), "nothing left to clear");
    let row = s.account(&a).unwrap().unwrap();
    assert_eq!(
        (row.quarantine_reason, row.quarantine_fp, row.quarantine_at),
        (None, None, None)
    );
    assert!(matches!(
        s.set_quarantine(
            &AccountId::from_string("nobody"),
            "invalid_grant",
            "sha256:x",
            1
        ),
        Err(StoreError::NoSuchAccount)
    ));
}

#[test]
fn login_expiry_is_recorded_on_its_own() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_login_expires_at(&a, Some(1_797_000_000_000)).unwrap();
    assert_eq!(
        s.account(&a).unwrap().unwrap().login_expires_at,
        Some(1_797_000_000_000)
    );
    s.set_login_expires_at(&a, None).unwrap();
    assert_eq!(s.account(&a).unwrap().unwrap().login_expires_at, None);
}

/// `path` with SQLite's sidecar suffix (`-wal`, `-shm`).
fn sidecar(path: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}{suffix}", path.display()))
}

#[test]
fn the_database_and_its_sidecars_are_created_0600() {
    // L421, §6.1: the store names every account, so it is private from creation.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("data/tagteam.db");
    let s = Store::open(&path).unwrap();
    s.set_active(&cc(), None, None).unwrap(); // a write: the WAL is in use
    for p in [path.clone(), sidecar(&path, "-wal"), sidecar(&path, "-shm")] {
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", p.display());
    }
}

/// The `switch` event a commit from `from` to `to` inserts.
fn switch_event(from: &AccountId, to: &AccountId, trigger: &str, source: &str) -> EventRow {
    EventRow {
        at: 1_790_000_000_000,
        provider: cc(),
        kind: "switch".into(),
        from_id: Some(from.clone()),
        to_id: Some(to.clone()),
        trigger: Some(trigger.into()),
        source: source.into(),
        detail: None,
    }
}

/// What an automatic switch from `from` to `to` records (§11.2 step 11).
fn record(from: &AccountId, to: &AccountId) -> AutoRecord {
    AutoRecord {
        at: 1_790_000_000,
        from: from.clone(),
        to: to.clone(),
        departure: Departure {
            left_headroom: Some(4.5),
            left_recovery_at: Some(1_790_009_630),
            left_trigger: Trigger::Proactive,
        },
    }
}

#[test]
fn auto_switch_state_is_the_default_until_written_and_kept_per_provider() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let fake = ProviderId::new("fake-agent");
    assert_eq!(s.autoswitch_state(&cc()).unwrap(), AutoState::default());
    s.set_unhealthy_ticks(&cc(), 2).unwrap();
    s.set_unhealthy_ticks(&fake, 1).unwrap();
    assert_eq!(
        s.autoswitch_state(&cc()).unwrap(),
        AutoState {
            unhealthy_ticks: 2,
            ..AutoState::default()
        }
    );
    s.set_unhealthy_ticks(&cc(), 3).unwrap();
    assert_eq!(s.autoswitch_state(&cc()).unwrap().unhealthy_ticks, 3);
    assert_eq!(s.autoswitch_state(&fake).unwrap().unhealthy_ticks, 1);
}

#[test]
fn an_automatic_commit_records_its_departure_and_resets_the_count() {
    // §9.4 step 9 and Decision 4: the record and the reset ride the switch's own commit.
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    s.set_unhealthy_ticks(&cc(), 2).unwrap();
    let ev = switch_event(&a, &b, "proactive", "auto");
    s.commit_switch(&cc(), &b, 0, &ev, Some(&record(&a, &b)))
        .unwrap();
    let recorded = AutoState {
        last_switch_at: Some(1_790_000_000),
        last_switch_from: Some(a.clone()),
        last_switch_to: Some(b.clone()),
        left_headroom: Some(4.5),
        left_recovery_at: Some(1_790_009_630),
        left_trigger: Some(Trigger::Proactive),
        unhealthy_ticks: 0,
    };
    assert_eq!(s.autoswitch_state(&cc()).unwrap(), recorded);
    assert_eq!(s.active(&cc()).unwrap(), Some(b.clone()));
    assert_eq!(s.events().unwrap(), vec![ev]);
    // A manual commit records nothing: the last automatic switch's record stands.
    s.set_unhealthy_ticks(&cc(), 1).unwrap();
    s.commit_switch(&cc(), &a, 0, &switch_event(&b, &a, "manual", "cli"), None)
        .unwrap();
    assert_eq!(
        s.autoswitch_state(&cc()).unwrap(),
        AutoState {
            unhealthy_ticks: 1,
            ..recorded
        }
    );
}

#[test]
fn the_record_lands_with_the_commit_or_not_at_all() {
    // One transaction (§9.4 step 9): a commit whose active-account write fails, here on its
    // foreign key, leaves no record, and the journal row stays for recovery.
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("t.db")).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    s.insert_journal(&JournalRow {
        provider: cc(),
        holder: ProcessStamp { pid: 1, start: 2 },
        from_id: Some(a.clone()),
        to_id: b.clone(),
        from_fp: None,
        from_identity: None,
        to_fp: "sha256:b".into(),
        started_at: 5,
        prior: None,
    })
    .unwrap();
    let ghost = AccountId::from_string("ghost");
    let ev = switch_event(&a, &ghost, "proactive", "auto");
    assert!(
        s.commit_switch(&cc(), &ghost, 0, &ev, Some(&record(&a, &ghost)))
            .is_err()
    );
    assert_eq!(s.autoswitch_state(&cc()).unwrap(), AutoState::default());
    assert!(s.journal(&cc()).unwrap().is_some());
    assert!(s.events().unwrap().is_empty());
}

#[test]
fn a_departure_trigger_this_build_does_not_know_reads_as_none() {
    // §11.3: a missing departure snapshot lifts the no-return bar; an unknown one is missing.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let ev = switch_event(&a, &b, "proactive", "auto");
    s.commit_switch(&cc(), &b, 0, &ev, Some(&record(&a, &b)))
        .unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE autoswitch_state SET left_trigger = 'idle-hold'", [])
        .unwrap();
    let state = s.autoswitch_state(&cc()).unwrap();
    assert_eq!(state.left_trigger, None);
    assert_eq!(state.last_switch_from, Some(a));
}

/// A `switch` event to `to`, as `commit_switch` records one.
fn switch_event_to(to: &AccountId) -> EventRow {
    EventRow {
        at: 6,
        provider: cc(),
        kind: "switch".into(),
        from_id: None,
        to_id: Some(to.clone()),
        trigger: Some("manual".into()),
        source: "cli".into(),
        detail: None,
    }
}

/// The provider's `active_accounts` row as stored, read through an independent connection.
fn active_row(path: &std::path::Path, provider: &str) -> Option<(Option<String>, Option<i64>)> {
    use rusqlite::OptionalExtension;
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT account_id, login_epoch FROM active_accounts WHERE provider = ?1",
            [provider],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .unwrap()
}

#[test]
fn a_version_1_store_is_migrated_with_its_activation_epochs_filled() {
    // Decision 1: `schema.sql` is the frozen v1 DDL, so a v1 file runs only the v2 step. The
    // backfill takes the live store as current (§12.5): each named account's activation epoch
    // is its current `login_epoch`. A journal row keeps a NULL `to_epoch` (§9.6).
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(include_str!("../src/store/schema.sql"))
            .unwrap();
        conn.execute_batch(
            "INSERT INTO accounts
               (id, provider, position, identity_key, label, kind, identity_json, login_epoch, added_at)
               VALUES ('a', 'claude-code', 1, 'a@x.co', 'a@x.co', 'oauth', '{}', 3, 1),
                      ('b', 'claude-code', 2, 'b@x.co', 'b@x.co', 'oauth', '{}', 0, 1);
             INSERT INTO active_accounts (provider, account_id)
               VALUES ('claude-code', 'a'), ('fake-agent', NULL);
             INSERT INTO switch_journal
               (provider, holder_pid, holder_start, from_id, to_id, to_fp, started_at)
               VALUES ('claude-code', 1, 2, 'a', 'b', 'sha256:b', 5);
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }

    let s = Store::open(&path).unwrap();

    assert_eq!(s.schema_version().unwrap(), 2);
    let a = AccountId::from_string("a");
    assert_eq!(
        s.activation(&cc()).unwrap(),
        Some(Activation {
            account: a.clone(),
            epoch: Some(3)
        })
    );
    assert!(
        !s.live_store_stale(&s.account(&a).unwrap().unwrap())
            .unwrap(),
        "the live store is taken as current"
    );
    assert_eq!(s.activation(&ProviderId::new("fake-agent")).unwrap(), None);
    assert_eq!(s.journal(&cc()).unwrap().unwrap().to_epoch, None);
    drop(s);
    assert_eq!(
        active_row(&path, "fake-agent"),
        Some((None, None)),
        "NULL only with account_id"
    );
    assert_eq!(journal_mode(&path), "wal");
    // Reopening runs nothing more: the file is already at the build's version.
    assert_eq!(Store::open(&path).unwrap().schema_version().unwrap(), 2);
}

#[test]
fn the_activation_epoch_moves_with_the_active_account() {
    // Both columns are written on every upsert: an epoch left from the previous account would
    // stale-mark the next one (§12.5).
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    let on = |account: &AccountId, epoch| {
        Some(Activation {
            account: account.clone(),
            epoch,
        })
    };
    assert_eq!(s.activation(&cc()).unwrap(), None);

    s.set_active(&cc(), Some(&a), Some(5)).unwrap();
    assert_eq!(s.activation(&cc()).unwrap(), on(&a, Some(5)));
    s.set_active(&cc(), Some(&b), None).unwrap();
    assert_eq!(
        s.activation(&cc()).unwrap(),
        on(&b, None),
        "a's epoch is not kept for b"
    );
    s.commit_switch(&cc(), &a, 2, &switch_event_to(&a), None)
        .unwrap();
    assert_eq!(s.activation(&cc()).unwrap(), on(&a, Some(2)));
    assert_eq!(
        s.active(&cc()).unwrap(),
        Some(a.clone()),
        "`active` is the id alone"
    );

    // No account, no epoch, whatever the caller passes.
    s.set_active(&cc(), None, Some(9)).unwrap();
    assert_eq!(s.activation(&cc()).unwrap(), None);
    assert_eq!(active_row(&path, "claude-code"), Some((None, None)));
}

#[test]
fn deleting_the_active_account_clears_its_epoch_too() {
    // `ON DELETE SET NULL` clears `account_id` only; §6.1 allows a NULL epoch only with it.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    s.set_active(&cc(), Some(&a), Some(4)).unwrap();

    s.delete_account(&a).unwrap();

    assert_eq!(s.activation(&cc()).unwrap(), None);
    assert_eq!(active_row(&path, "claude-code"), Some((None, None)));
}

#[test]
fn the_live_store_is_stale_only_when_the_active_account_s_epoch_moved() {
    // §12.5: stale-marked when `active_accounts` names the account with an activation epoch
    // other than its `login_epoch`. A row with no epoch is no evidence either way.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE accounts SET login_epoch = 2 WHERE id = 'a'", [])
        .unwrap();
    let stale = |id: Option<&AccountId>, epoch: Option<i64>| {
        s.set_active(&cc(), id, epoch).unwrap();
        s.live_store_stale(&s.account(&a).unwrap().unwrap())
            .unwrap()
    };

    assert!(!stale(None, None), "no active account");
    assert!(!stale(Some(&b), Some(0)), "another account is active");
    assert!(!stale(Some(&a), Some(2)), "activated at its current epoch");
    assert!(stale(Some(&a), Some(1)), "activated before a replacement");
    assert!(!stale(Some(&a), None), "no epoch recorded");
}

#[test]
fn a_prior_snapshot_from_before_to_epoch_reads_as_none() {
    // §9.6: a row written before the column has no epoch of its own. Its `prior` snapshot has
    // no `to_epoch` key at all, and still parses.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("t.db");
    let s = Store::open(&path).unwrap();
    let a = add(&s, &cc(), "a", "a@x.co", 1);
    let b = add(&s, &cc(), "b", "b@x.co", 2);
    s.insert_journal(&JournalRow {
        provider: cc(),
        holder: ProcessStamp { pid: 1, start: 2 },
        from_id: Some(a),
        to_id: b,
        from_fp: None,
        from_identity: None,
        to_fp: "sha256:b".into(),
        to_epoch: Some(1),
        started_at: 5,
        prior: None,
    })
    .unwrap();
    drop(s);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE switch_journal SET prior = ?1 WHERE provider = 'claude-code'",
            [r#"{"provider":"claude-code","holder_pid":1,"holder_start":2,"from_id":"a","to_id":"b","from_fp":null,"from_identity":null,"to_fp":"sha256:b","started_at":4,"prior":null}"#],
        )
        .unwrap();

    let row = Store::open(&path).unwrap().journal(&cc()).unwrap().unwrap();

    assert_eq!(row.to_epoch, Some(1));
    assert_eq!(row.prior.unwrap().to_epoch, None);
}
