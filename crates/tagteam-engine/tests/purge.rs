//! §10.5 and B.66: `purge` deletes tagteam's data account by account, never a live login,
//! refuses while anything it would delete is in use, and is finished by running it again.
mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{FakeFx, Fx, add, crashed_switch, credential, journal, vault_fp};
use serde_json::json;
use tagteam_cc::live::Platform;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::purge::{KEYCHAIN_LEFTOVERS, PurgeAccount, PurgePlan};
use tagteam_engine::store::{DisplacedRow, JournalRow, Store};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::liveness::FakeProcess;
use tagteam_provider::profile::ProfileMarker;
use tagteam_provider::{FlockGuard, Keychain, KeychainError, LockState, ProcessStamp, Read};

/// The account a plan or report names, as `purge` builds it.
fn account(fx: &Fx, id: &AccountId, has_profile: bool) -> PurgeAccount {
    let row = fx.engine.store().unwrap().account(id).unwrap().unwrap();
    PurgeAccount {
        id: row.id,
        provider: row.provider,
        position: row.position,
        label: row.label,
        has_profile,
    }
}

/// A displaced entry of `provider`, file and row, as `displace` leaves one (§6.3).
fn plant_displaced(fx: &Fx, provider: &ProviderId, id: &str, email: &str) -> PathBuf {
    let dir = fx.env.data_dir().join("displaced");
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{id}.json"));
    fs::write(&file, credential(email, "rt-displaced")).unwrap();
    fx.engine
        .store()
        .unwrap()
        .insert_displaced(&DisplacedRow {
            id: id.into(),
            provider: provider.clone(),
            at: 1_790_000_000_000,
            reason: "displaced-live-login".into(),
            fingerprint: "sha256:00".into(),
            identity: Some(json!({"email": email})),
        })
        .unwrap();
    file
}

/// The log and its two rotations, and the rotation lock beside them (§14.2).
fn plant_log(fx: &Fx) -> [PathBuf; 4] {
    let dir = fx.env.state_dir();
    fs::create_dir_all(&dir).unwrap();
    let files = [
        "tagteam.log",
        "tagteam.log.1",
        "tagteam.log.2",
        "tagteam.log.lock",
    ]
    .map(|name| dir.join(name));
    for f in &files {
        fs::write(f, "line\n").unwrap();
    }
    files
}

/// Rows of `table` for `provider`, read behind the store's back.
fn count(fx: &Fx, table: &str, provider: &str) -> i64 {
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE provider = ?1"),
            [provider],
            |r| r.get(0),
        )
        .unwrap()
}

/// The live login as Claude Code holds it: its credential item and `~/.claude.json`.
fn live_login(fx: &Fx) -> (Option<serde_json::Value>, Vec<u8>) {
    (
        fx.live_credential(),
        fs::read(fx.paths().global_config).unwrap(),
    )
}

/// `id`'s profile under `sessions/`, with its own hashed Keychain item (§12.2).
fn profile_with_item(fx: &Fx, id: &AccountId) -> (PathBuf, (String, String)) {
    let dir = fx.make_profile(id);
    let item = fx.profile_item(&dir);
    fx.kc
        .put(&item.0, &item.1, &credential("a@x.co", "rt-profile"));
    (dir, item)
}

/// A profile directory under `sessions/` that no stored account owns, whose marker names
/// `owner` (an account that is gone).
fn orphan(fx: &Fx, owner: &str) -> PathBuf {
    let dir = fx.env.data_dir().join("sessions").join(owner);
    fx.write_marker(&dir, &AccountId::from_string(owner), &fx.env);
    dir
}

/// Holds `provider`'s engine lock with a record naming `pid`, as a running `auto` does (§11.1).
fn running_engine(fx: &Fx, provider: &str, pid: u32) -> FlockGuard {
    let path = fx
        .env
        .data_dir()
        .join("locks")
        .join(format!("autoswitch-{provider}.lock"));
    let held = FlockGuard::try_lock(&path).unwrap().unwrap();
    fs::write(&path, format!("{{\"pid\":{pid},\"start\":1}}\n")).unwrap();
    held
}

fn full_plan(fx: &Fx) -> PurgePlan {
    fx.engine.purge_plan(None).unwrap()
}

#[test]
fn planning_creates_nothing_and_names_what_a_full_purge_deletes() {
    let fx = Fx::new();
    let empty = full_plan(&fx);
    assert!(!fx.env.data_dir().exists(), "§5: a plan creates nothing");
    assert_eq!(
        empty,
        PurgePlan {
            provider: None,
            accounts: vec![],
            orphan_profiles: vec![],
            rescues: 0,
            displaced: 0,
            store_and_log: true,
            keychain_orphans: false,
        }
    );
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    fx.make_profile(&a);
    let gone = orphan(&fx, "0192-gone");
    fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a2"));
    plant_displaced(
        &fx,
        &fx.provider(),
        "1790000000-aaaaaaaaaaaa-abc123",
        "x@x.co",
    );
    let plan = full_plan(&fx);
    assert_eq!(
        plan,
        PurgePlan {
            provider: None,
            accounts: vec![account(&fx, &a, true), account(&fx, &b, false)],
            orphan_profiles: vec![gone],
            rescues: 1,
            displaced: 1,
            store_and_log: true,
            keychain_orphans: false,
        }
    );
    let narrowed = fx.engine.purge_plan(Some(&fx.provider())).unwrap();
    assert_eq!(
        (narrowed.store_and_log, narrowed.rescues, narrowed.displaced),
        (false, 1, 1)
    );
}

#[test]
fn a_purge_that_finds_no_data_directory_creates_none() {
    // §5: nothing to delete there, so no lock file either; the log and the Keychain are apart.
    let fx = Fx::new();
    let [log, ..] = plant_log(&fx);
    fx.kc.put(SERVICE, "another-stores-account", b"theirs");
    let report = fx.engine.purge(&full_plan(&fx)).unwrap();
    assert!(!fx.env.data_dir().exists());
    assert!(!log.exists());
    assert!(report.store_emptied);
    assert_eq!(report.warnings, [KEYCHAIN_LEFTOVERS]);
    let narrowed = fx.engine.purge_plan(Some(&fx.provider())).unwrap();
    assert_eq!(fx.engine.purge(&narrowed).unwrap(), Default::default());
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn a_full_purge_deletes_everything_but_the_settings_the_locks_and_the_live_login() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b"); // live
    let (profile, item) = profile_with_item(&fx, &a);
    fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a2"));
    let displaced = plant_displaced(
        &fx,
        &fx.provider(),
        "1790000000-aaaaaaaaaaaa-abc123",
        "x@x.co",
    );
    let config = fx.env.config_dir().join("config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "[autoswitch]\nthreshold = 80\n").unwrap();
    let [log, log1, log2, rotation_lock] = plant_log(&fx);
    // Another data directory's account, in the Keychain every data directory shares.
    fx.kc.put(SERVICE, "another-stores-account", b"theirs");
    let live = live_login(&fx);

    let report = fx.engine.purge(&full_plan(&fx)).unwrap();

    assert_eq!(
        report.accounts,
        vec![
            account_like(&a, 1, "a@x.co", true),
            account_like(&b, 2, "b@x.co", false)
        ]
    );
    assert_eq!((report.rescues, report.displaced), (1, 1));
    assert!(report.store_emptied);
    assert_eq!(report.failures, Vec::<(String, String)>::new());
    assert_eq!(report.warnings, [KEYCHAIN_LEFTOVERS]);
    // Every item of its own accounts is gone; another data directory's is not.
    let left: Vec<String> = fx
        .kc
        .items()
        .into_keys()
        .filter(|(svc, _)| svc == SERVICE)
        .map(|(_, acct)| acct)
        .collect();
    assert_eq!(left, ["another-stores-account"]);
    assert!(
        fx.kc.get(&item.0, &item.1).is_none(),
        "the profile's hashed item"
    );
    assert!(!profile.exists());
    assert!(!fx.env.data_dir().join("rescue").exists());
    assert!(!displaced.exists());
    // The store is empty, and still a store.
    let store = Store::open_existing(&fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    assert_eq!(store.schema_version().unwrap(), 2);
    assert!(store.all_accounts().unwrap().is_empty());
    assert!(store.events().unwrap().is_empty());
    assert!(store.displaced_rows().unwrap().is_empty());
    // Kept: the user's settings and every lock file.
    assert!(config.exists());
    for lock in [
        fx.env.data_dir().join(".mutation.lock"),
        fx.env.data_dir().join(format!("locks/{a}.lock")),
        fx.env.data_dir().join("locks/autoswitch-claude-code.lock"),
        rotation_lock,
    ] {
        assert!(lock.exists(), "{}", lock.display());
    }
    for gone in [log, log1, log2] {
        assert!(!gone.exists(), "{}", gone.display());
    }
    assert_eq!(
        live_login(&fx),
        live,
        "B.66: the live login is never touched"
    );
}

/// `PurgeAccount` with the fields a test knows.
fn account_like(id: &AccountId, position: u32, label: &str, has_profile: bool) -> PurgeAccount {
    PurgeAccount {
        id: id.clone(),
        provider: ProviderId::new("claude-code"),
        position,
        label: label.into(),
        has_profile,
    }
}

#[test]
fn keychain_orphans_deletes_every_tagteam_item_and_warns_of_none() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fx.kc.put(SERVICE, "another-stores-account", b"theirs");
    fx.kc.put(SERVICE, "another-stores-account.prev", b"theirs");
    let plan = PurgePlan {
        keychain_orphans: true,
        ..full_plan(&fx)
    };
    let report = fx.engine.purge(&plan).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(fx.kc.items().keys().all(|(svc, _)| svc != SERVICE));
}

#[test]
fn keychain_orphans_with_a_provider_is_refused_before_anything() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let plan = PurgePlan {
        keychain_orphans: true,
        ..fx.engine.purge_plan(Some(&fx.provider())).unwrap()
    };
    let err = fx.engine.purge(&plan).unwrap_err();
    assert_eq!(err.kind(), "invalid-input", "{err}");
    assert!(fx.vault_bytes(&a).is_some());
}

#[test]
fn no_deleted_row_survives_in_the_store_file_or_its_wal() {
    // Decision 8: `secure_delete`, a rebuild, then a checkpoint that truncates the WAL. The
    // replacement rewrote alice's row before the purge, without `secure_delete`.
    let fx = Fx::new();
    fx.add("alice.secret@x.co", "rt-a");
    fx.add("alice.secret@x.co", "rt-a2"); // a replacement rewrites the row
    fx.add("bob.secret@x.co", "rt-b");
    plant_displaced(
        &fx,
        &fx.provider(),
        "1790000000-aaaaaaaaaaaa-abc123",
        "carol.secret@x.co",
    );
    let files = |fx: &Fx| -> Vec<u8> {
        ["tagteam.db", "tagteam.db-wal"]
            .iter()
            .filter_map(|f| fs::read(fx.env.data_dir().join(f)).ok())
            .flatten()
            .collect()
    };
    let holds =
        |bytes: &[u8], needle: &str| bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
    let before = files(&fx);
    for email in ["alice.secret", "bob.secret", "carol.secret"] {
        assert!(
            holds(&before, email),
            "{email} is in the store to begin with"
        );
    }
    fx.engine.purge(&full_plan(&fx)).unwrap();
    let after = files(&fx);
    for email in ["alice.secret", "bob.secret", "carol.secret"] {
        assert!(!holds(&after, email), "{email} survived the purge");
    }
    let wal = fx.env.data_dir().join("tagteam.db-wal");
    assert!(
        fs::metadata(&wal).map_or(true, |m| m.len() == 0),
        "the WAL is truncated"
    );
}

#[test]
fn on_linux_a_full_purge_deletes_the_vault_directory() {
    let fx = Fx::with_platform(Platform::Linux);
    fx.add("a@x.co", "rt-a");
    let vault = fx.env.data_dir().join("vault");
    fs::write(vault.join("left-by-a-store-deleted-by-hand.json"), "{}").unwrap();
    let report = fx.engine.purge(&full_plan(&fx)).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(!vault.exists());
}

#[test]
fn a_provider_purge_keeps_the_other_provider_s_data_and_the_usage_budget() {
    let ffx = FakeFx::new();
    let fx = &ffx.fx;
    let cc = fx.provider();
    let fake = ffx.fake_provider();
    let a = fx.add("a@x.co", "rt-a");
    let h = ffx.fake_add("hank", "tok-h", "renew-h");
    let ours = plant_displaced(fx, &cc, "1790000000-aaaaaaaaaaaa-abc123", "x@x.co");
    let theirs = plant_displaced(fx, &fake, "1790000001-bbbbbbbbbbbb-def456", "hank");
    let unrecorded = fx
        .env
        .data_dir()
        .join("displaced/1790000002-cccccccccccc-ghi789.json");
    fs::write(&unrecorded, "{}").unwrap();
    let store = fx.engine.store().unwrap();
    for p in [&cc, &fake] {
        store.set_unhealthy_ticks(p, 2).unwrap();
    }
    let db = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    for p in ["claude-code", "fake-agent"] {
        db.execute(
            "INSERT INTO live_identity_cache (provider, identity_key) VALUES (?1, 'k')",
            [p],
        )
        .unwrap();
        db.execute(
            "INSERT INTO usage_requests (provider, identity_key, at) VALUES (?1, 'k', 1)",
            [p],
        )
        .unwrap();
    }
    // FakeAgent's switch in flight, by a live process: not this purge's to settle.
    store
        .insert_journal(&JournalRow {
            provider: fake.clone(),
            holder: ProcessStamp::current().unwrap(),
            from_id: None,
            to_id: h.clone(),
            from_fp: None,
            from_identity: None,
            to_fp: "sha256:00".into(),
            to_epoch: None,
            started_at: 1,
            prior: None,
        })
        .unwrap();

    let plan = ffx.engine.purge_plan(Some(&cc)).unwrap();
    assert_eq!(plan.accounts.len(), 1);
    let report = ffx.engine.purge(&plan).unwrap();
    assert_eq!((report.accounts.len(), report.displaced), (1, 1));
    assert!(report.failures.is_empty(), "{:?}", report.failures);

    assert!(store.account(&a).unwrap().is_none());
    assert!(fx.vault_bytes(&a).is_none());
    assert!(!ours.exists() && theirs.exists() && unrecorded.exists());
    for table in [
        "events",
        "autoswitch_state",
        "active_accounts",
        "live_identity_cache",
        "displaced",
    ] {
        assert_eq!(count(fx, table, "claude-code"), 0, "{table}");
        assert!(count(fx, table, "fake-agent") > 0, "{table}");
    }
    assert_eq!(count(fx, "switch_journal", "fake-agent"), 1);
    assert_eq!(
        (
            count(fx, "usage_requests", "claude-code"),
            count(fx, "usage_requests", "fake-agent")
        ),
        (1, 1),
        "§8.6: the budget does not reset"
    );
    assert!(store.account(&h).unwrap().is_some());
    assert!(fx.kc.get(SERVICE, h.as_str()).is_some());
}

#[test]
fn inside_a_run_shell_neither_plans_nor_purges() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let plan = full_plan(&fx);
    let dir = fx.make_profile(&a);
    let shell = fx.engine_located(fx.shell_env(&dir));
    assert_eq!(
        shell.purge_plan(None).unwrap_err().kind(),
        "inside-run-shell"
    );
    assert_eq!(shell.purge(&plan).unwrap_err().kind(), "inside-run-shell");
    assert!(fx.vault_bytes(&a).is_some());
}

#[test]
fn a_running_engine_refuses_naming_its_pid() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let plan = full_plan(&fx);
    let engine = running_engine(&fx, "claude-code", 4321);
    let err = fx.engine.purge(&plan).unwrap_err();
    assert!(
        matches!(&err, EngineError::EngineRunning { provider, pid: Some(4321) } if provider == "claude-code"),
        "{err:?}"
    );
    assert_eq!(err.kind(), "engine-running");
    assert!(err.to_string().contains("pid 4321"), "{err}");
    assert!(fx.vault_bytes(&a).is_some());
    drop(engine);
    fx.engine.purge(&plan).unwrap();
}

#[test]
fn a_session_owned_account_refuses_and_nothing_is_deleted() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let dir = fx.make_profile(&b);
    let plan = full_plan(&fx);
    let session = fx.hold_reservation(&dir);
    let err = fx.engine.purge(&plan).unwrap_err();
    assert!(
        matches!(err, EngineError::SessionOwned { position: 2, .. }),
        "{err:?}"
    );
    assert!(fx.vault_bytes(&a).is_some(), "refused before any account");
    drop(session);
    fx.live_record(&dir, 777, "bg");
    assert_eq!(fx.engine.purge(&plan).unwrap_err().kind(), "session-owned");
}

#[test]
fn an_orphaned_profile_in_use_refuses() {
    // Review Focus 3: the profile's account row is gone, and a session still runs in it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let gone = orphan(&fx, "0192-gone");
    let plan = full_plan(&fx);
    let session = fx.hold_reservation(&gone);
    let err = fx.engine.purge(&plan).unwrap_err();
    assert!(
        matches!(&err, EngineError::OrphanSessionRunning { profile } if profile == &gone),
        "{err:?}"
    );
    assert_eq!(err.kind(), "session-owned");
    assert!(fx.vault_bytes(&a).is_some() && gone.exists());
    drop(session);
    // A live record counts too, and so does one that cannot be read.
    let record = fx.live_record(&gone, 778, "interactive");
    assert_eq!(fx.engine.purge(&plan).unwrap_err().kind(), "session-owned");
    fs::write(&record, "{\"pid\": ").unwrap();
    assert_eq!(fx.engine.purge(&plan).unwrap_err().kind(), "session-owned");
    fs::remove_file(&record).unwrap();
    // An unreadable marker counts as affected even by a `--provider` purge.
    fs::write(gone.join(".tagteam-profile.json"), "not a marker").unwrap();
    let narrowed = fx.engine.purge_plan(Some(&fx.provider())).unwrap();
    assert_eq!(narrowed.orphan_profiles, [gone.clone()]);
    let _session = fx.hold_reservation(&gone);
    assert_eq!(
        fx.engine.purge(&narrowed).unwrap_err().kind(),
        "session-owned"
    );
}

#[test]
fn an_orphaned_profile_holding_real_history_refuses_as_an_account_s_does() {
    // §12.2: a readable marker names the orphan's provider, whose share lists judge it as
    // `remove` judges an account's profile. Nothing is deleted.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let gone = orphan(&fx, "0192-gone");
    let projects = gone.join("projects");
    fs::create_dir(&projects).unwrap();
    let err = fx.engine.purge(&full_plan(&fx)).unwrap_err();
    assert!(
        matches!(&err, EngineError::ProfileSplit { profile, .. } if profile == &projects),
        "{err:?}"
    );
    assert_eq!(err.kind(), "profile-split");
    assert!(
        err.to_string()
            .starts_with(&format!("{} is a real copy where ", projects.display())),
        "{err}"
    );
    assert!(fx.vault_bytes(&a).is_some() && projects.exists());
}

#[test]
fn orphaned_profiles_go_with_their_items_and_another_provider_s_stays() {
    let ffx = FakeFx::new();
    let fx = &ffx.fx;
    fx.add("a@x.co", "rt-a");
    // Readable marker: the item its recorded spelling names.
    let readable = orphan(fx, "0192-gone");
    let readable_item = fx.profile_item(&readable);
    fx.kc.put(&readable_item.0, &readable_item.1, b"{}");
    // Unreadable marker: the item its canonical path names.
    let broken = fx.env.data_dir().join("sessions/0192-broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join(".tagteam-profile.json"), "not a marker").unwrap();
    let canonical = fs::canonicalize(&broken).unwrap();
    let broken_item = fx.item_for_spelling(canonical.to_str().unwrap());
    fx.kc.put(&broken_item.0, &broken_item.1, b"{}");
    // FakeAgent's orphan: not affected by a Claude Code purge.
    let theirs = fx.env.data_dir().join("sessions/0192-fake");
    fx.make_profile_for(ffx.fake.as_ref(), &AccountId::from_string("0192-fake"));
    let plan = ffx.engine.purge_plan(Some(&fx.provider())).unwrap();
    assert_eq!(plan.orphan_profiles, [broken.clone(), readable.clone()]);
    let report = ffx.engine.purge(&plan).unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(!readable.exists() && !broken.exists());
    assert!(fx.kc.get(&readable_item.0, &readable_item.1).is_none());
    assert!(fx.kc.get(&broken_item.0, &broken_item.1).is_none());
    assert!(theirs.exists());
}

#[test]
fn accounts_added_or_removed_since_the_plan_refuse() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let plan = full_plan(&fx);
    let c = fx.add("c@x.co", "rt-c");
    let err = fx.engine.purge(&plan).unwrap_err();
    assert_eq!(err.kind(), "purge-changed", "{err}");
    assert!(fx.vault_bytes(&a).is_some() && fx.vault_bytes(&c).is_some());
    let plan = full_plan(&fx);
    fx.engine.remove(&c).unwrap();
    assert_eq!(fx.engine.purge(&plan).unwrap_err().kind(), "purge-changed");
    assert!(fx.vault_bytes(&a).is_some());
}

#[test]
fn a_rescue_file_is_deleted_by_a_full_purge_and_refused_by_remove_and_a_provider_purge() {
    // Review Focus 3, and §6.3: a `rescue` that is not a directory hides every account's.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let rescue = fx.env.data_dir().join("rescue");
    fs::write(&rescue, "not a directory").unwrap();
    let narrowed = fx.engine.purge_plan(Some(&fx.provider())).unwrap();
    let err = fx.engine.purge(&narrowed).unwrap_err();
    assert!(
        matches!(&err, EngineError::RescueUnlistable { path, .. } if path == &rescue),
        "{err:?}"
    );
    assert_eq!(err.kind(), "rescue-unreadable");
    assert_eq!(
        fx.engine.remove(&a).unwrap_err().kind(),
        "rescue-unreadable"
    );
    assert!(fx.vault_bytes(&a).is_some(), "refused before the vault");
    assert!(rescue.is_file());
    let plan = full_plan(&fx);
    assert_eq!(plan.rescues, 1);
    let report = fx.engine.purge(&plan).unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!((report.accounts.len(), report.rescues), (1, 1));
    assert!(!rescue.exists());
    assert!(fx.vault_bytes(&a).is_none());
}

#[test]
fn an_undecidable_interrupted_switch_is_deleted_with_a_warning_and_the_live_login_kept() {
    // Decision 7: purge recovers what it can, and deletes what it cannot decide.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    fx.rotate_live("rt-nobody-knows"); // neither side's generation, and no oracle
    let live = live_login(&fx);
    let plan = fx.engine.purge_plan(Some(&fx.provider())).unwrap();
    let report = fx.engine.purge(&plan).unwrap();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("may be incoherent"),
        "{:?}",
        report.warnings
    );
    assert!(
        fx.engine
            .store()
            .unwrap()
            .journal(&fx.provider())
            .unwrap()
            .is_none()
    );
    assert_eq!(report.accounts.len(), 2);
    assert_eq!(live_login(&fx), live);
}

#[test]
fn a_journal_row_that_does_not_decode_is_deleted_as_one_recovery_cannot_decide() {
    // §10.5 step 5, Decision 7: purge never refuses on an interrupted switch, and a row it
    // cannot even read is one recovery cannot decide. Every other reader still refuses it.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db"))
        .unwrap()
        .execute("UPDATE switch_journal SET prior = 'not json'", [])
        .unwrap();
    assert!(
        fx.engine.store().unwrap().journals().is_err(),
        "`journals` stays strict"
    );
    let live = live_login(&fx);
    let plan = fx.engine.purge_plan(Some(&fx.provider())).unwrap();
    let report = fx.engine.purge(&plan).unwrap();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("may be incoherent"),
        "{:?}",
        report.warnings
    );
    assert_eq!(count(&fx, "switch_journal", "claude-code"), 0);
    assert_eq!(report.accounts.len(), 2);
    assert_eq!(live_login(&fx), live);
}

#[test]
fn a_profile_holding_real_history_refuses_the_purge_as_it_refuses_remove() {
    // §10.3 Guard and §12.2: real history is never deleted or split silently. Purge asks what
    // `remove` asks (M4a's `refuse_profile_split`) and says what `remove` says.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&a);
    fs::create_dir(dir.join("projects")).unwrap();
    let err = fx.engine.purge(&full_plan(&fx)).unwrap_err();
    assert_eq!(err.kind(), "profile-split", "{err}");
    assert_eq!(
        err.to_string(),
        fx.engine.remove(&a).unwrap_err().to_string()
    );
    let narrowed = fx.engine.purge_plan(Some(&fx.provider())).unwrap();
    assert_eq!(
        fx.engine.purge(&narrowed).unwrap_err().kind(),
        "profile-split"
    );
    assert!(
        fx.vault_bytes(&a).is_some() && dir.join("projects").exists(),
        "refused before anything"
    );
}

#[test]
fn a_purge_that_cannot_delete_an_account_keeps_the_store_and_the_next_one_finishes_it() {
    // §10.5: "running purge or remove again finishes it", from the account's row, and "no
    // consumed generation is left where a newer one was deleted": the vault keeps generation
    // G, so `rescue` keeps its successor. A full purge keeps the store whole, the rescue path
    // and the vault's leftovers; a provider's purge keeps that provider's rows, and deletes an
    // account's rescue files only after its vault, as `remove` does.
    for full in [true, false] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let successor = fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a2"));
        let plan_of = |fx: &Fx| match full {
            true => full_plan(fx),
            false => fx.engine.purge_plan(Some(&fx.provider())).unwrap(),
        };
        let plan = plan_of(&fx);
        fx.kc.set_fail_delete(SERVICE, true);
        let report = fx.engine.purge(&plan).unwrap();
        fx.kc.set_fail_delete(SERVICE, false);
        assert!(
            report.accounts.is_empty() && !report.store_emptied && report.rescues == 0,
            "{report:?}"
        );
        let store = match full {
            true => "the store",
            false => "the store's rows for claude-code",
        };
        let what: Vec<&str> = report.failures.iter().map(|(w, _)| w.as_str()).collect();
        assert_eq!(what, ["claude-code #1 (a@x.co)", store], "full: {full}");
        let kept = &report.failures[1].1;
        assert!(
            kept.contains("since 1 account could not be deleted"),
            "{kept}"
        );
        assert_eq!(kept.contains("the rescue path"), full, "{kept}");
        assert!(
            fx.vault_bytes(&a).is_some() && successor.exists(),
            "G and its successor stay together"
        );
        assert!(fx.engine.store().unwrap().account(&a).unwrap().is_some());
        assert_eq!(
            count(&fx, "active_accounts", "claude-code"),
            1,
            "full: {full}"
        );
        let again = plan_of(&fx);
        assert_eq!(again.accounts, plan.accounts, "a new plan names it");
        let report = fx.engine.purge(&again).unwrap();
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert_eq!(
            (report.accounts.len(), report.rescues, report.store_emptied),
            (1, 1, full)
        );
        assert!(fx.vault_bytes(&a).is_none() && !successor.exists());
        assert!(
            fx.engine
                .store()
                .unwrap()
                .all_accounts()
                .unwrap()
                .is_empty()
        );
    }
}

/// A profile directory whose marker names `provider` and `id`, as a tagteam that registers
/// `provider` writes one (§12.2).
fn marked_for(dir: &std::path::Path, provider: &ProviderId, id: &AccountId) {
    ProfileMarker {
        provider: provider.clone(),
        account_id: id.clone(),
        config_dir: dir.display().to_string(),
        outer: json!({}),
    }
    .write(dir)
    .unwrap();
}

/// An account of `provider`, which this build does not register, as a tagteam that does
/// leaves one: its row, its vault entry, a rescue file, and a session profile whose marker
/// names it, with a link out of it to a directory that must survive. Returns the account, its
/// profile, its rescue file and the link's target.
fn unregistered_account(fx: &Fx, provider: &ProviderId) -> (AccountId, PathBuf, PathBuf, PathBuf) {
    let id = add(
        &fx.engine.store().unwrap(),
        provider,
        "0192-ghost",
        "g@x.co",
        1,
    );
    fx.put_vault(&id, b"a credential only its provider can read");
    let rescue = fx.plant_rescue(&id, "sha256:00", &credential("g@x.co", "rt-g"));
    let profile = fx.profile_dir(&id);
    marked_for(&profile, provider, &id);
    let outside = fx.env.home.join("kept-outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("kept"), "x").unwrap();
    std::os::unix::fs::symlink(&outside, profile.join("linked")).unwrap();
    (id, profile, rescue, outside)
}

#[test]
fn a_full_purge_deletes_an_account_of_a_provider_this_build_does_not_register() {
    // §10.5: purge is the way out of a state tagteam cannot repair, as an account that another
    // tagteam's provider added is to this one. Everything goes but the profile's credential
    // item, which only that provider can name, and a warning says so.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let ghost = ProviderId::new("ghost");
    let (g, profile, rescue, outside) = unregistered_account(&fx, &ghost);
    // And a profile no account owns whose marker names that provider.
    let stray = fx.env.data_dir().join("sessions").join("0192-ghost-gone");
    marked_for(&stray, &ghost, &AccountId::from_string("0192-ghost-gone"));
    let plan = full_plan(&fx);
    assert_eq!(
        plan.accounts,
        [account(&fx, &a, false), account(&fx, &g, true)],
        "listed with its provider's ID"
    );
    assert_eq!(plan.accounts[1].provider, ghost);
    assert_eq!(plan.orphan_profiles, [stray.clone()]);

    let report = fx.engine.purge(&plan).unwrap();

    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.accounts, plan.accounts);
    assert_eq!(report.rescues, 1);
    assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
    for (warning, names) in report.warnings.iter().zip(["ghost #1", "0192-ghost-gone"]) {
        assert!(
            warning.contains(names) && warning.contains("cannot be named"),
            "{warning}"
        );
    }
    assert!(fx.vault_bytes(&g).is_none() && fx.vault_bytes(&a).is_none());
    assert!(!rescue.exists());
    assert!(!profile.exists() && !stray.exists());
    assert!(
        outside.join("kept").exists(),
        "a link in the profile is removed as a link"
    );
    let store = fx.engine.store().unwrap();
    assert!(store.all_accounts().unwrap().is_empty());
}

#[test]
fn a_provider_purge_names_a_provider_this_build_does_not_register_while_it_has_accounts() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let ghost = ProviderId::new("ghost");
    let (g, profile, rescue, _) = unregistered_account(&fx, &ghost);
    assert_eq!(
        fx.engine
            .purge_plan(Some(&ProviderId::new("nobody")))
            .unwrap_err()
            .kind(),
        "unknown-provider",
        "a name the store holds no account of"
    );
    let plan = fx.engine.purge_plan(Some(&ghost)).unwrap();
    assert_eq!(plan.accounts, [account(&fx, &g, true)]);
    assert_eq!(plan.rescues, 1);
    // B.66: a session in its profile refuses, naming the account. A registered provider with
    // sessions judges the profile, since its own provider cannot be asked.
    let session = fx.hold_reservation(&profile);
    let err = fx.engine.purge(&plan).unwrap_err();
    assert!(
        matches!(err, EngineError::SessionOwned { position: 1, .. }),
        "{err:?}"
    );
    assert!(fx.vault_bytes(&g).is_some(), "refused before anything");
    drop(session);
    let report = fx.engine.purge(&plan).unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.accounts, plan.accounts);
    assert!(fx.vault_bytes(&g).is_none() && !rescue.exists() && !profile.exists());
    assert_eq!(count(&fx, "events", "ghost"), 0, "its other rows go too");
    assert!(
        fx.vault_bytes(&a).is_some(),
        "another provider's account stays"
    );
    assert_eq!(
        fx.engine.purge_plan(Some(&ghost)).unwrap_err().kind(),
        "unknown-provider",
        "once its accounts are gone"
    );
}

#[test]
fn an_orphan_whose_marker_cannot_be_read_refuses_when_it_holds_real_history() {
    // §12.2: no share list is known for it, so every registered provider's must-share entries
    // are checked, and a real one refuses, by a full purge and by a provider's alike.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let broken = fx.env.data_dir().join("sessions/0192-broken");
    let projects = broken.join("projects");
    fs::create_dir_all(&projects).unwrap();
    fs::write(broken.join(".tagteam-profile.json"), "not a marker").unwrap();
    let narrowed = fx.engine.purge_plan(Some(&fx.provider())).unwrap();
    assert_eq!(narrowed.orphan_profiles, [broken.clone()]);
    for plan in [full_plan(&fx), narrowed] {
        let err = fx.engine.purge(&plan).unwrap_err();
        assert!(
            matches!(&err, EngineError::ProfileSplit { profile, .. } if profile == &projects),
            "{err:?}"
        );
        assert_eq!(err.kind(), "profile-split");
    }
    assert!(fx.vault_bytes(&a).is_some() && projects.exists());
}

#[test]
fn an_account_of_a_provider_this_build_does_not_register_refuses_when_it_holds_real_history() {
    // §12.2: its provider's share lists cannot be asked, so every registered provider's
    // must-share entries are checked; its link out of the profile is no history, a real
    // `projects/` is.
    let fx = Fx::new();
    let ghost = ProviderId::new("ghost");
    let (g, profile, rescue, _) = unregistered_account(&fx, &ghost);
    let projects = profile.join("projects");
    fs::create_dir(&projects).unwrap();
    for plan in [full_plan(&fx), fx.engine.purge_plan(Some(&ghost)).unwrap()] {
        let err = fx.engine.purge(&plan).unwrap_err();
        assert!(
            matches!(&err, EngineError::ProfileSplit { profile, .. } if profile == &projects),
            "{err:?}"
        );
    }
    assert!(fx.vault_bytes(&g).is_some() && rescue.exists() && projects.exists());
    assert!(
        fx.engine.store().unwrap().account(&g).unwrap().is_some(),
        "nothing is deleted"
    );
}

#[test]
fn only_a_dead_writer_s_temp_files_in_tagteam_s_own_directories_go() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fx.process.set(
        777,
        FakeProcess {
            exists: Some(true),
            ..FakeProcess::default()
        },
    );
    let config = fx.env.config_dir();
    fs::create_dir_all(&config).unwrap();
    let dead = fx.env.data_dir().join(".tagteam.db.tagteam-4242-0badf00d");
    let live = config.join(".config.toml.tagteam-777-12345678");
    let beside_cc = fx
        .env
        .home
        .join(".claude/.credentials.json.tagteam-4242-0badf00d");
    for f in [&dead, &live, &beside_cc] {
        fs::write(f, "partial").unwrap();
    }
    fx.engine.purge(&full_plan(&fx)).unwrap();
    assert!(!dead.exists());
    assert!(live.exists(), "its writer may still publish it");
    assert!(beside_cc.exists(), "§3: outside tagteam's own directories");
}

#[cfg(feature = "test-hooks")]
#[test]
fn a_purge_interrupted_after_each_account_is_finished_by_running_it_again() {
    // B.66: each account's vault goes first, so what is left is never usable, and a new plan
    // names exactly what is left.
    let fx = Fx::new();
    let ids = [
        fx.add("a@x.co", "rt-a"),
        fx.add("b@x.co", "rt-b"),
        fx.add("c@x.co", "rt-c"),
    ];
    for (done, id) in ids.iter().enumerate() {
        let plan = full_plan(&fx);
        assert_eq!(plan.accounts.len(), ids.len() - done);
        fx.engine.fail_at(Some("purge-account-deleted"));
        assert!(fx.engine.purge(&plan).is_err());
        fx.engine.fail_at(None);
        assert!(fx.vault_bytes(id).is_none());
        assert!(fx.engine.store().unwrap().account(id).unwrap().is_none());
        for later in &ids[done + 1..] {
            assert!(fx.vault_bytes(later).is_some());
        }
    }
    let report = fx.engine.purge(&full_plan(&fx)).unwrap();
    assert!(report.accounts.is_empty() && report.store_emptied);
}

#[cfg(feature = "test-hooks")]
#[test]
fn an_add_waiting_on_the_purge_lands_in_the_emptied_store() {
    // §10.5 step 4: the add waits for `MutationGuard`, which the purge holds to the end.
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    use tagteam_engine::lifecycle::AddOptions;

    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.login("c@x.co", "rt-c");
    let plan = full_plan(&fx);
    let adder = Arc::new(fx.engine_with_env(fx.env.clone()));
    let (reached, waiting) = mpsc::channel();
    let (reached, waiting) = (Mutex::new(reached), Mutex::new(waiting));
    adder.on_point(
        "before-mutation-lock",
        Box::new(move || {
            let _ = reached.lock().unwrap().send(());
        }),
    );
    let started = Arc::new(Mutex::new(None));
    let (add, handle) = (adder.clone(), started.clone());
    fx.engine.on_point(
        "purge-guarded",
        Box::new(move || {
            let add = add.clone();
            let opts = AddOptions {
                provider: ProviderId::new("claude-code"),
                position: None,
                alias: None,
                yes: false,
            };
            *handle.lock().unwrap() = Some(std::thread::spawn(move || add.add_live(opts)));
            waiting
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .expect("the add reached the mutation lock");
        }),
    );
    let report = fx.engine.purge(&plan).unwrap();
    assert_eq!(report.accounts.len(), 2);
    let added = started
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .join()
        .unwrap()
        .unwrap()
        .account;
    let store = fx.engine.store().unwrap();
    let rows = store.all_accounts().unwrap();
    assert_eq!(rows.len(), 1, "only the add's account");
    assert_eq!(
        (rows[0].id.clone(), rows[0].position),
        (added.id.clone(), 1)
    );
    assert!(fx.vault_bytes(&added.id).is_some());
}

#[cfg(feature = "test-hooks")]
#[test]
fn a_store_an_add_creates_after_purge_found_no_data_directory_is_refused_never_emptied() {
    // §5 and §10.5 step 6: finding no data directory, a full purge runs without a lock, so it
    // never opens, creates or empties a store. One an `add` creates meanwhile takes the guarded
    // path, which refuses the account the confirmed plan did not name.
    use std::sync::{Arc, Mutex};

    use tagteam_engine::lifecycle::AddOptions;

    let fx = Fx::new();
    fx.login("a@x.co", "rt-a");
    let plan = full_plan(&fx);
    assert!(plan.accounts.is_empty() && !fx.env.data_dir().exists());
    let adder = Arc::new(fx.engine_with_env(fx.env.clone()));
    let added = Arc::new(Mutex::new(None));
    let slot = added.clone();
    fx.engine.on_point(
        "purge-without-data-dir",
        Box::new(move || {
            let opts = AddOptions {
                provider: ProviderId::new("claude-code"),
                position: None,
                alias: None,
                yes: false,
            };
            *slot.lock().unwrap() = Some(adder.add_live(opts).unwrap().account.id);
        }),
    );
    let err = fx.engine.purge(&plan).unwrap_err();
    assert_eq!(err.kind(), "purge-changed", "{err}");
    let id = added.lock().unwrap().clone().expect("the add ran");
    assert!(fx.vault_bytes(&id).is_some());
    assert!(
        fx.engine.store().unwrap().account(&id).unwrap().is_some(),
        "the store keeps the account"
    );
}

#[test]
fn a_provider_purge_whose_second_displaced_entry_fails_counts_the_first_and_reports_the_failure() {
    // `purge_displaced` stops at its first failure and the entries before it stay deleted: they
    // are purged, so the report counts them, and the failure is reported with its cause.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let (first, second) = (
        "1790000001-bbbbbbbbbbbb-def456",
        "1790000000-aaaaaaaaaaaa-abc123",
    );
    let first_file = plant_displaced(&fx, &fx.provider(), first, "x@x.co");
    // Newest first, ties by ID: `first` goes before `second`. A directory where `second`'s file
    // should be makes its deletion fail.
    let second_file = plant_displaced(&fx, &fx.provider(), second, "y@x.co");
    fs::remove_file(&second_file).unwrap();
    fs::create_dir(&second_file).unwrap();
    fs::write(second_file.join("held"), "x").unwrap();

    let plan = fx.engine.purge_plan(Some(&fx.provider())).unwrap();
    let report = fx.engine.purge(&plan).unwrap();

    assert_eq!(report.displaced, 1, "{report:?}");
    assert!(!first_file.exists() && second_file.exists());
    let what: Vec<&str> = report.failures.iter().map(|(w, _)| w.as_str()).collect();
    assert_eq!(what, ["displaced credentials"], "{:?}", report.failures);
    assert_eq!(count(&fx, "displaced", "claude-code"), 1, "its row stays");
    assert_eq!(report.accounts.len(), 1, "the accounts went regardless");
}

#[test]
fn a_refused_purge_deletes_no_undecidable_journal_row() {
    // Fix round 1, I1: step 5 recovers, but a leftover row is deleted only once step 6's
    // refusals have passed, so a refused purge leaves the interrupted switch for the next
    // guarded command to report. A purge that goes ahead deletes it with its warning.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    fx.rotate_live("rt-nobody-knows"); // neither side's generation, and no oracle
    let dir = fx.make_profile(&a);
    let plan = fx.engine.purge_plan(Some(&fx.provider())).unwrap();
    let session = fx.hold_reservation(&dir);
    let err = fx.engine.purge(&plan).unwrap_err();
    assert_eq!(err.kind(), "session-owned", "{err}");
    assert!(journal(&fx).is_some(), "a refused purge deletes nothing");
    assert!(fx.vault_bytes(&a).is_some());
    drop(session);
    let report = fx.engine.purge(&plan).unwrap();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].contains("may be incoherent"),
        "{:?}",
        report.warnings
    );
    assert!(journal(&fx).is_none());
}

#[cfg(feature = "test-hooks")]
#[test]
fn a_provider_purge_that_found_no_data_directory_checks_again_and_hands_over() {
    // Fix round 1, M2: the check after "no data directory" holds for `--provider` too. A data
    // directory an `add` made meanwhile is the guarded path's, where step 6 refuses the
    // account the confirmed plan did not name.
    use std::sync::{Arc, Mutex};

    use tagteam_engine::lifecycle::AddOptions;

    let fx = Fx::new();
    fx.login("a@x.co", "rt-a");
    let plan = fx.engine.purge_plan(Some(&fx.provider())).unwrap();
    assert!(plan.accounts.is_empty() && !fx.env.data_dir().exists());
    let adder = Arc::new(fx.engine_with_env(fx.env.clone()));
    let added = Arc::new(Mutex::new(None));
    let slot = added.clone();
    fx.engine.on_point(
        "purge-without-data-dir",
        Box::new(move || {
            let opts = AddOptions {
                provider: ProviderId::new("claude-code"),
                position: None,
                alias: None,
                yes: false,
            };
            *slot.lock().unwrap() = Some(adder.add_live(opts).unwrap().account.id);
        }),
    );
    let err = fx.engine.purge(&plan).unwrap_err();
    assert_eq!(err.kind(), "purge-changed", "{err}");
    let id = added.lock().unwrap().clone().expect("the add ran");
    assert!(fx.vault_bytes(&id).is_some());
    assert!(fx.engine.store().unwrap().account(&id).unwrap().is_some());
}

/// `from` copied to `to`, files and directories (not links).
fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[test]
fn a_copy_of_an_account_s_profile_never_deletes_the_account_s_item() {
    // Fix round 1, M3: an orphan whose readable marker names an account the store still holds
    // is a copy, not that account's profile. Step 8 names its item by its own path, so the
    // account's item survives when step 7 could not delete the account.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let (original, item) = profile_with_item(&fx, &a);
    let copy = fx.env.data_dir().join("sessions/copy");
    copy_dir(&original, &copy);
    let plan = full_plan(&fx);
    assert_eq!(plan.orphan_profiles, [copy.clone()]);
    fx.kc.set_fail_delete(SERVICE, true);
    let report = fx.engine.purge(&plan).unwrap();
    fx.kc.set_fail_delete(SERVICE, false);
    assert!(report.accounts.is_empty(), "{report:?}");
    assert!(!copy.exists(), "the copy itself goes");
    assert!(original.exists());
    assert!(
        fx.kc.get(&item.0, &item.1).is_some(),
        "the account's own item survives step 8"
    );
}

#[test]
fn an_orphan_naming_the_live_config_dir_never_deletes_the_live_login() {
    // Fix round 1, M4 (§10.5: purge never deletes the live login): an orphan link with no
    // marker whose canonical path is the directory `CLAUDE_CONFIG_DIR` names would be given
    // that directory's item, which is the live login's.
    let fx = Fx::with(tagteam_cc::live::Platform::MacOs, |e| {
        let parent = fs::canonicalize(e.home.parent().unwrap()).unwrap();
        let live = parent.join(e.home.file_name().unwrap()).join(".claude");
        e.claude_config_dir = Some(live.into_os_string());
    });
    fx.add("a@x.co", "rt-a");
    let live_dir = PathBuf::from(fx.env.claude_config_dir.clone().unwrap());
    let live_item = fx.live_item(tagteam_cc::ItemKind::OAuth);
    fx.kc.put(&live_item.0, &live_item.1, b"the live login");
    let link = fx.env.data_dir().join("sessions/live-link");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&live_dir, &link).unwrap();
    assert_eq!(fs::canonicalize(&link).unwrap(), live_dir);

    let plan = full_plan(&fx);
    assert_eq!(plan.orphan_profiles, [link.clone()]);
    let report = fx.engine.purge(&plan).unwrap();

    assert_eq!(
        fx.kc.get(&live_item.0, &live_item.1).as_deref(),
        Some(&b"the live login"[..])
    );
    assert!(live_dir.join("settings.json").exists(), "nor its directory");
    let what: Vec<&str> = report.failures.iter().map(|(w, _)| w.as_str()).collect();
    assert_eq!(what, [link.display().to_string()], "{:?}", report.failures);
    assert!(
        report.failures[0].1.contains("live login"),
        "{:?}",
        report.failures
    );
}

#[test]
fn a_stored_provider_string_never_places_a_lock_outside_locks() {
    // Fix round 1, M5: a provider ID is a stored string. One that is not a plain name gets a
    // lock file name derived from its hash, inside `locks/`.
    let fx = Fx::new();
    let hostile = ProviderId::new("/../../escape");
    add(
        &fx.engine.store().unwrap(),
        &hostile,
        "0192-hostile",
        "h@x.co",
        1,
    );
    let data = fx.env.data_dir();
    let report = fx.engine.purge(&full_plan(&fx)).unwrap();
    assert_eq!(report.accounts.len(), 1, "{report:?}");
    assert!(!data.join("escape.lock").exists());
    let names: Vec<String> = fs::read_dir(data.join("locks"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("engine-"))
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].ends_with(".lock"));
    assert!(
        data.join("locks/autoswitch-claude-code.lock").exists(),
        "a registered provider keeps its name"
    );
    for entry in fs::read_dir(&data).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        assert!(!name.contains("escape"), "{name}");
    }
}

/// A Keychain that forwards to `FakeKeychain` and counts the deletes by service it is asked.
struct CountingKeychain {
    inner: Arc<tagteam_provider::FakeKeychain>,
    service_deletes: Arc<AtomicUsize>,
}

impl Keychain for CountingKeychain {
    fn find(&self, s: &str, a: &str) -> Read<Vec<u8>> {
        self.inner.find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> Read<()> {
        self.inner.exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), KeychainError> {
        self.inner.upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), KeychainError> {
        self.inner.delete(s, a)
    }
    fn lock_state(&self) -> LockState {
        self.inner.lock_state()
    }
    fn unlock(&self) -> bool {
        self.inner.unlock()
    }
    fn delete_service(&self, s: &str) -> Result<u32, KeychainError> {
        self.service_deletes.fetch_add(1, Ordering::SeqCst);
        self.inner.delete_service(s)
    }
    fn service_has_items(&self, s: &str) -> Read<bool> {
        self.inner.service_has_items(s)
    }
}

#[test]
fn keychain_orphans_deletes_nothing_by_service_while_an_account_is_left() {
    // Fix round 1, M7: an account left over may own an entry the sweep would delete.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.kc.put(SERVICE, "another-stores-account", b"theirs");
    let calls = Arc::new(AtomicUsize::new(0));
    let vault = tagteam_engine::vault::Vault::new(Box::new(
        tagteam_engine::vault::KeychainVault::new(Arc::new(CountingKeychain {
            inner: fx.kc.clone(),
            service_deletes: calls.clone(),
        })),
    ));
    let engine = fx.engine_with_vault(vault);
    let plan = PurgePlan {
        keychain_orphans: true,
        ..engine.purge_plan(None).unwrap()
    };
    fx.kc.set_fail_delete(SERVICE, true);
    let report = engine.purge(&plan).unwrap();
    fx.kc.set_fail_delete(SERVICE, false);
    assert!(
        report.accounts.is_empty() && !report.store_emptied,
        "{report:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no delete by service");
    assert!(fx.vault_bytes(&a).is_some());
    assert!(fx.kc.get(SERVICE, "another-stores-account").is_some());
    // Finished, the sweep runs once.
    let again = PurgePlan {
        keychain_orphans: true,
        ..engine.purge_plan(None).unwrap()
    };
    engine.purge(&again).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn on_linux_a_purge_that_leaves_an_account_keeps_the_vault_directory() {
    // Fix round 1, M7: `vault/` holds the left account's entry.
    let fx = Fx::with_platform(Platform::Linux);
    let a = fx.add("a@x.co", "rt-a");
    let vault = fx.env.data_dir().join("vault");
    // A directory where the account's entry should be makes its deletion fail.
    let entry = vault.join(format!("{a}.json"));
    fs::remove_file(&entry).unwrap();
    fs::create_dir(&entry).unwrap();
    fs::write(entry.join("held"), "x").unwrap();
    let report = fx.engine.purge(&full_plan(&fx)).unwrap();
    assert!(
        report.accounts.is_empty() && !report.store_emptied,
        "{report:?}"
    );
    assert!(vault.exists(), "kept with the account that needs it");
    assert!(report.failures.iter().any(|(w, _)| w == "the store"));
}

#[test]
fn a_link_to_an_account_s_profile_never_deletes_the_account_s_item() {
    // Fix round 2, M3: `sessions/x -> sessions/<a>` reads `a`'s marker through the link, and
    // its canonical path is `a`'s own profile. Its item is `a`'s, so with `a` left by step 7
    // the orphan is skipped whole.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let (original, item) = profile_with_item(&fx, &a);
    let link = fx.env.data_dir().join("sessions/x");
    std::os::unix::fs::symlink(&original, &link).unwrap();
    let plan = full_plan(&fx);
    assert_eq!(plan.orphan_profiles, [link.clone()]);
    fx.kc.set_fail_delete(SERVICE, true);
    let report = fx.engine.purge(&plan).unwrap();
    fx.kc.set_fail_delete(SERVICE, false);
    assert!(report.accounts.is_empty(), "{report:?}");
    assert!(fx.kc.get(&item.0, &item.1).is_some(), "a's item survives");
    assert!(original.exists());
    assert!(
        fs::symlink_metadata(&link).is_ok(),
        "the orphan is skipped whole"
    );
    assert!(
        report
            .failures
            .iter()
            .any(|(w, m)| w == &link.display().to_string() && m.contains("stored account")),
        "{:?}",
        report.failures
    );
}

#[test]
fn an_orphan_named_by_the_secure_storage_override_never_deletes_the_live_login() {
    // Fix round 2, M4: `CLAUDE_SECURESTORAGE_CONFIG_DIR` names the live items before
    // `CLAUDE_CONFIG_DIR` does (Appendix A.2), so an orphan at that spelling holds the live item.
    let fx = Fx::with(tagteam_cc::live::Platform::MacOs, |e| {
        let root = e.home.parent().unwrap().to_path_buf();
        let canonical_root = fs::canonicalize(&root).unwrap();
        let orphan = canonical_root
            .join(e.data_dir().strip_prefix(&root).unwrap())
            .join("sessions/secure");
        e.claude_securestorage_config_dir = Some(orphan.into_os_string());
    });
    let orphan = fx.env.data_dir().join("sessions/secure");
    fs::create_dir_all(&orphan).unwrap();
    let live_item = fx.live_item(tagteam_cc::ItemKind::OAuth);
    fx.kc.put(&live_item.0, &live_item.1, b"the live login");
    let plan = full_plan(&fx);
    assert_eq!(plan.orphan_profiles, [orphan.clone()]);
    let report = fx.engine.purge(&plan).unwrap();
    assert_eq!(
        fx.kc.get(&live_item.0, &live_item.1).as_deref(),
        Some(&b"the live login"[..])
    );
    assert!(orphan.exists(), "skipped whole");
    assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
    let message = &report.failures[0].1;
    assert!(
        message.contains("live login") && message.contains("CLAUDE_CONFIG_DIR"),
        "names the provider's own variable: {message}"
    );
}
