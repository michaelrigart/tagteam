//! `displaced/` (§6.3): the listing joins rows and files, a purge deletes the file and then the
//! row under the displaced lock, and a displaced row names only the live login's own secret
//! (L467).

mod common;

use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use common::{API_KEY, Fx, STRAY_API_KEY, crashed_switch, write_target_credential};
use serde_json::{Value, json};
use tagteam_cc::ItemKind;
use tagteam_core::ProviderId;
use tagteam_engine::displace::{DISPLACED_LOCK, DisplacedEntry, DisplacedList};
use tagteam_engine::store::DisplacedRow;
use tagteam_provider::{FlockGuard, Keychain};

/// Three entries' IDs, newest first by the time each carries.
const NEWEST: &str = "1790000300-0123456789ab-aaaaaa";
const MIDDLE: &str = "1790000250-fedcba987654-bbbbbb";
const OLDEST: &str = "1790000200-00112233aabb-cccccc";

fn dir(fx: &Fx) -> PathBuf {
    fx.env.data_dir().join("displaced")
}

/// The fingerprint a planted row records: its ID's 12 hex digits, then zeros.
fn fingerprint(id: &str) -> String {
    format!("sha256:{}{}", id.split('-').nth(1).unwrap(), "0".repeat(52))
}

/// A file in `displaced/` named `name`, as `displace` leaves one (or anything else there).
fn plant_file(fx: &Fx, name: &str) {
    fs::create_dir_all(dir(fx)).unwrap();
    fs::write(dir(fx).join(name), format!("credential of {name}")).unwrap();
}

/// A row as `displace` records one.
fn plant_row(fx: &Fx, id: &str, at: i64, identity: Option<Value>) {
    fx.engine
        .store()
        .unwrap()
        .insert_displaced(&DisplacedRow {
            id: id.into(),
            provider: fx.provider(),
            at,
            reason: "displaced-live-login".into(),
            fingerprint: fingerprint(id),
            identity,
        })
        .unwrap();
}

/// `a` and `b` stored at 1 and 2, and three entries:
/// - NEWEST: a row and its file, naming `b`;
/// - MIDDLE: a file with no row;
/// - OLDEST: a row whose file is gone, naming a login tagteam does not manage.
fn three_entries() -> Fx {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    plant_file(&fx, &format!("{NEWEST}.json"));
    plant_row(
        &fx,
        NEWEST,
        1_790_000_300_500,
        Some(Fx::oauth_account("b@x.co")),
    );
    plant_file(&fx, &format!("{MIDDLE}.json"));
    plant_row(
        &fx,
        OLDEST,
        1_790_000_200_000,
        Some(Fx::oauth_account("stranger@x.co")),
    );
    fx
}

#[test]
fn the_listing_joins_rows_and_files_newest_first() {
    let fx = three_entries();
    // Decision 12: a name that is not `<displaced ID>.json` is never listed. That covers the
    // atomic writer's temp files, another case, another suffix, and no suffix.
    for foreign in [
        "notes.txt",
        ".1790000400-0123456789ab-dddddd.json.tagteam-42-0000abcd",
        "1790000400-0123456789AB-dddddd.json",
        "1790000400-0123456789ab-dddddd.json.bak",
        "1790000400-0123456789ab-dddddd",
    ] {
        plant_file(&fx, foreign);
    }
    let cc = Some(fx.provider());
    assert_eq!(
        fx.engine.displaced().unwrap(),
        DisplacedList {
            dir: dir(&fx),
            entries: vec![
                DisplacedEntry {
                    id: NEWEST.into(),
                    provider: cc.clone(),
                    at_ms: 1_790_000_300_500,
                    reason: Some("displaced-live-login".into()),
                    fingerprint: Some(fingerprint(NEWEST)),
                    identity: Some(Fx::oauth_account("b@x.co")),
                    account: Some(2),
                    file_present: true,
                    recorded: true,
                },
                // Unrecorded: the time its name carries, and nothing else known.
                DisplacedEntry {
                    id: MIDDLE.into(),
                    provider: None,
                    at_ms: 1_790_000_250_000,
                    reason: None,
                    fingerprint: None,
                    identity: None,
                    account: None,
                    file_present: true,
                    recorded: false,
                },
                DisplacedEntry {
                    id: OLDEST.into(),
                    provider: cc,
                    at_ms: 1_790_000_200_000,
                    reason: Some("displaced-live-login".into()),
                    fingerprint: Some(fingerprint(OLDEST)),
                    identity: Some(Fx::oauth_account("stranger@x.co")),
                    account: None,
                    file_present: false,
                    recorded: true,
                },
            ],
        }
    );
}

#[test]
fn account_is_the_managed_account_the_identity_names_unless_its_uuid_conflicts() {
    // §6.1: an identity key names an account, and an account uuid that conflicts makes it a
    // different one, such as a recycled email. A missing uuid is no conflict.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let cases = [
        (
            json!({"emailAddress": "a@x.co", "organizationUuid": "", "accountUuid": "uuid-a@x.co"}),
            Some(1),
        ),
        (
            json!({"emailAddress": "a@x.co", "organizationUuid": ""}),
            Some(1),
        ),
        (
            json!({"emailAddress": "a@x.co", "organizationUuid": "", "accountUuid": "uuid-recycled"}),
            None,
        ),
        (
            json!({"emailAddress": "a@x.co", "organizationUuid": "org-1"}),
            None,
        ),
        (json!({"organizationUuid": ""}), None),
    ];
    let id = |i: usize| format!("179000000{i}-0123456789ab-aaaaaa");
    for (i, (identity, _)) in cases.iter().enumerate() {
        plant_row(
            &fx,
            &id(i),
            1_790_000_000_000 + i as i64,
            Some(identity.clone()),
        );
    }
    // A provider this build does not register cannot read its identity. An empty
    // fingerprint is none.
    fx.engine
        .store()
        .unwrap()
        .insert_displaced(&DisplacedRow {
            id: id(9),
            provider: ProviderId::new("fake-agent"),
            at: 1,
            reason: "displaced-live-login".into(),
            fingerprint: String::new(),
            identity: Some(cases[0].0.clone()),
        })
        .unwrap();
    let list = fx.engine.displaced().unwrap();
    let entry = |i: usize| list.entries.iter().find(|e| e.id == id(i)).unwrap();
    for (i, (identity, account)) in cases.iter().enumerate() {
        assert_eq!(entry(i).account, *account, "{identity}");
        assert_eq!(entry(i).identity.as_ref(), Some(identity));
    }
    assert_eq!(
        (entry(9).account, entry(9).fingerprint.as_deref()),
        (None, None)
    );
}

#[test]
fn a_fresh_home_lists_nothing_and_creates_nothing() {
    // §5: a command that changes nothing creates nothing.
    let fx = Fx::new();
    assert_eq!(
        fx.engine.displaced().unwrap(),
        DisplacedList {
            dir: dir(&fx),
            entries: Vec::new(),
        }
    );
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn a_directory_that_cannot_be_listed_is_an_error_not_an_empty_listing() {
    // §4.3: unreadable is never collapsed into absent.
    let fx = three_entries();
    fs::set_permissions(dir(&fx), Permissions::from_mode(0o000)).unwrap();
    let result = fx.engine.displaced();
    fs::set_permissions(dir(&fx), Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result.unwrap_err().kind(), "io");
}

/// Holds the displaced lock as another tagteam process would.
fn hold_lock(fx: &Fx) -> FlockGuard {
    FlockGuard::try_lock(&fx.env.data_dir().join(DISPLACED_LOCK))
        .unwrap()
        .expect("the displaced lock is free")
}

#[test]
fn purge_deletes_each_entry_file_and_row() {
    // §6.3: a row with its file, a file with no row, and a row whose file is gone.
    let fx = three_entries();
    plant_file(&fx, "notes.txt");
    let ids = [NEWEST, MIDDLE, OLDEST].map(str::to_owned);
    assert_eq!(fx.engine.purge_displaced(&ids).unwrap(), ids);
    assert!(fx.engine.displaced().unwrap().entries.is_empty());
    assert!(
        fx.engine
            .store()
            .unwrap()
            .displaced_rows()
            .unwrap()
            .is_empty()
    );
    let left: Vec<_> = fs::read_dir(dir(&fx))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["notes.txt"], "only what was never an entry is left");
}

#[test]
fn a_repeated_id_is_deleted_once() {
    let fx = three_entries();
    let ids = [NEWEST, NEWEST].map(str::to_owned);
    assert_eq!(fx.engine.purge_displaced(&ids).unwrap(), [NEWEST]);
}

#[test]
fn an_id_that_names_no_entry_is_refused_and_nothing_is_deleted() {
    // Review Focus 3, Decision 12. An ID is checked before any path is built from it, and
    // every ID before anything is deleted, so a valid one alongside it is kept too. A file
    // named in upper case exists, and is still no entry. It has no lowercase twin among the
    // fixtures, so it is a file of its own on a case-insensitive filesystem too.
    let fx = three_entries();
    plant_file(&fx, "1790000100-0123456789AB-dddddd.json");
    let before = fx.engine.displaced().unwrap();
    for bad in [
        "../../tagteam.db",
        "x",
        "",
        "1790000100-0123456789AB-dddddd",
        "1790000300-0123456789ab-AAAAAA",
        "1790000999-0123456789ab-zzzzzz",
    ] {
        for ids in [[NEWEST, bad], [bad, NEWEST]] {
            let ids = ids.map(str::to_owned);
            let err = fx.engine.purge_displaced(&ids).unwrap_err();
            assert_eq!(err.kind(), "no-such-displaced", "{ids:?}");
            assert_eq!(
                err.to_string(),
                format!("no displaced credential matches {bad:?}; `tagteam displaced` lists them")
            );
        }
    }
    assert_eq!(
        fx.engine.displaced().unwrap(),
        before,
        "nothing was deleted"
    );
    assert!(
        dir(&fx)
            .join("1790000100-0123456789AB-dddddd.json")
            .exists()
    );
    assert!(fx.env.data_dir().join("tagteam.db").exists());
}

#[test]
fn a_purge_deletes_a_symlink_itself_and_never_what_it_points_to() {
    // §6.3: the path is deleted, not its target.
    let fx = three_entries();
    let outside = fx.env.data_dir().join("outside.txt");
    fs::write(&outside, "not a displaced credential").unwrap();
    let link = dir(&fx).join(format!("{MIDDLE}.json"));
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    plant_row(&fx, MIDDLE, 1_790_000_250_000, None);
    assert_eq!(
        fx.engine.purge_displaced(&[MIDDLE.to_owned()]).unwrap(),
        [MIDDLE]
    );
    assert!(fs::symlink_metadata(&link).is_err(), "the link is gone");
    assert_eq!(fs::read(&outside).unwrap(), b"not a displaced credential");
    assert!(
        fx.engine
            .store()
            .unwrap()
            .displaced_rows()
            .unwrap()
            .iter()
            .all(|r| r.id != MIDDLE)
    );
}

#[test]
fn a_row_with_malformed_identity_json_can_still_be_purged_by_its_id() {
    // The listing is strict about a row it cannot read, but the purge needs only the ID, so a
    // damaged row never blocks its own deletion.
    let fx = three_entries();
    fx.engine.store().unwrap();
    let id = "1790000050-aabbccddeeff-eeeeee";
    let conn = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    conn.execute(
        "INSERT INTO displaced (id, provider, at, reason, fingerprint, identity) \
         VALUES (?1, 'claude-code', 1, 'displaced-live-login', '', 'not json')",
        [id],
    )
    .unwrap();
    drop(conn);
    assert!(fx.engine.displaced().is_err());
    assert_eq!(fx.engine.purge_displaced(&[id.to_owned()]).unwrap(), [id]);
    let conn = rusqlite::Connection::open(fx.env.data_dir().join("tagteam.db")).unwrap();
    let left: i64 = conn
        .query_row("SELECT count(*) FROM displaced WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(left, 0);
    assert_eq!(fx.engine.displaced().unwrap().entries.len(), 3);
}

#[test]
fn purging_on_a_fresh_home_refuses_and_creates_nothing() {
    let fx = Fx::new();
    let err = fx.engine.purge_displaced(&[NEWEST.to_owned()]).unwrap_err();
    assert_eq!(err.kind(), "no-such-displaced");
    assert!(!fx.env.data_dir().exists());
}

#[test]
fn a_file_that_cannot_be_deleted_keeps_its_row() {
    // §6.3: the file goes first, verified gone, and only then the row. A credential still on
    // disk never loses the row that names it.
    let fx = three_entries();
    fs::set_permissions(dir(&fx), Permissions::from_mode(0o500)).unwrap();
    let result = fx.engine.purge_displaced(&[NEWEST.to_owned()]);
    fs::set_permissions(dir(&fx), Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result.unwrap_err().kind(), "io");
    let newest = fx.engine.displaced().unwrap().entries.remove(0);
    assert_eq!(
        (newest.id.as_str(), newest.recorded, newest.file_present),
        (NEWEST, true, true)
    );
}

#[test]
fn a_purge_that_stops_part_way_reports_what_it_deleted_and_names_the_failed_path() {
    // §6.3: a purge stops at the first failing entry; the ones before it stay deleted, and the
    // error says which, and where it failed.
    let fx = three_entries();
    let stuck = dir(&fx).join(format!("{MIDDLE}.json"));
    fs::remove_file(&stuck).unwrap();
    fs::create_dir(&stuck).unwrap();
    let err = fx
        .engine
        .purge_displaced(&[NEWEST.to_owned(), MIDDLE.to_owned(), OLDEST.to_owned()])
        .unwrap_err();
    assert_eq!(err.kind(), "io");
    assert_eq!(err.deleted, [NEWEST]);
    assert!(err.to_string().contains(&format!("{MIDDLE}.json")), "{err}");
    assert!(!dir(&fx).join(format!("{NEWEST}.json")).exists());
    // The entry after the failure was never reached.
    let rows = fx.engine.store().unwrap().displaced_rows().unwrap();
    assert!(rows.iter().any(|r| r.id == OLDEST));
}

#[test]
fn a_purge_waits_for_the_displaced_lock() {
    // §6.3: each deletion holds the displaced lock, so a purge never deletes a file whose row
    // a writer is still to insert. Here another process holds the lock for 300 ms.
    let fx = three_entries();
    let held = hold_lock(&fx);
    let file = dir(&fx).join(format!("{NEWEST}.json"));
    let file_ref = &file;
    let present_while_held = thread::scope(|s| {
        let other = s.spawn(move || {
            thread::sleep(Duration::from_millis(300));
            let present = file_ref.exists();
            drop(held);
            present
        });
        assert_eq!(
            fx.engine.purge_displaced(&[NEWEST.to_owned()]).unwrap(),
            [NEWEST]
        );
        other.join().unwrap()
    });
    assert!(
        present_while_held,
        "the purge deleted the file under another process's lock"
    );
    assert!(!file.exists());
}

#[test]
fn the_writer_holds_the_displaced_lock_around_the_file_and_its_row() {
    // §6.3: a switch displacing the stray key waits for the lock before writing anything.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b");
    fx.put_managed_key(STRAY_API_KEY.as_bytes());
    let held = hold_lock(&fx);
    let displaced = dir(&fx);
    let displaced_ref = &displaced;
    let written_while_held = thread::scope(|s| {
        let other = s.spawn(move || {
            thread::sleep(Duration::from_millis(300));
            let written = fs::read_dir(displaced_ref).map_or(0, |d| d.count());
            drop(held);
            written
        });
        fx.switch_to(&a, false).unwrap();
        other.join().unwrap()
    });
    assert_eq!(
        written_while_held, 0,
        "the file was written under another process's lock"
    );
    assert_eq!(fx.displaced(), [STRAY_API_KEY.as_bytes()]);
}

#[test]
fn the_listing_takes_no_lock() {
    // §6.3: it reads the store and the directory only. A reader that waited on the held lock
    // would time out after 5 s.
    let fx = three_entries();
    let _held = hold_lock(&fx);
    let started = Instant::now();
    assert_eq!(fx.engine.displaced().unwrap().entries.len(), 3);
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// Every entry with its file's bytes, newest first.
fn with_bytes(fx: &Fx) -> Vec<(DisplacedEntry, Vec<u8>)> {
    let DisplacedList { dir, entries } = fx.engine.displaced().unwrap();
    entries
        .into_iter()
        .map(|e| {
            let bytes = fs::read(dir.join(format!("{}.json", e.id))).unwrap();
            (e, bytes)
        })
        .collect()
}

#[test]
fn a_stray_key_a_switch_clears_is_displaced_with_no_identity() {
    // §6.3, Decision 13. The managed key sits beside b's OAuth login, on the other auth axis
    // (§9.4 step 7). It is not b's, so its row names no identity, and so no account.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.add("b@x.co", "rt-b"); // live: b
    fx.put_managed_key(STRAY_API_KEY.as_bytes());
    fx.switch_to(&a, false).unwrap();
    let saved = with_bytes(&fx);
    assert_eq!(saved.len(), 1);
    let (entry, bytes) = &saved[0];
    assert_eq!(bytes.as_slice(), STRAY_API_KEY.as_bytes());
    assert_eq!((entry.identity.as_ref(), entry.account), (None, None));
    assert_eq!(entry.reason.as_deref(), Some("displaced-live-login"));
}

#[test]
fn a_forced_switch_names_the_live_login_on_its_own_secret_only() {
    // §9.4 step 2 displaces both axes. The stranger's OAuth credential is the live login's own,
    // so its row keeps the live identity. The key beside it gets none.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fx.login("stranger@x.co", "rt-s");
    fx.put_managed_key(STRAY_API_KEY.as_bytes());
    fx.switch_to(&a, true).unwrap();
    let saved = with_bytes(&fx);
    assert_eq!(saved.len(), 2);
    let identity_of = |secret: &[u8]| {
        saved
            .iter()
            .find(|(_, bytes)| bytes.as_slice() == secret)
            .map(|(e, _)| e.identity.clone())
            .expect("displaced")
    };
    let login = Fx::credential_json("stranger@x.co", "rt-s")
        .to_string()
        .into_bytes();
    assert_eq!(
        identity_of(&login),
        Some(Fx::oauth_account("stranger@x.co"))
    );
    assert_eq!(identity_of(STRAY_API_KEY.as_bytes()), None);
    assert!(
        saved.iter().all(|(e, _)| {
            e.account.is_none() && e.reason.as_deref() == Some("forced-activation")
        })
    );
}

#[test]
fn forward_recovery_displaces_a_stray_key_with_no_identity() {
    // §9.6 applies §9.4 step 7's rule before clearing the other axis. The key written between
    // the journal row and the crash is saved, attributed to no one.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &b, &a);
    write_target_credential(&fx, &a);
    fx.put_managed_key(STRAY_API_KEY.as_bytes());
    drop(fx.engine.mutation_guard().unwrap());
    let saved = with_bytes(&fx);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].1.as_slice(), STRAY_API_KEY.as_bytes());
    assert_eq!(saved[0].0.identity, None);
}

#[test]
fn a_stray_login_beside_an_api_key_account_whose_key_is_gone_gets_no_identity() {
    // Decision 13. The live account is an API-key account, so its own secret is on the
    // managed-key axis, and that key is gone. The OAuth login in the credential entry is not
    // its own: its row names no identity, never the API-key account's. A forced switch saves
    // it at §9.4 step 2, an unforced one at step 7.
    for (force, reason) in [(true, "forced-activation"), (false, "displaced-live-login")] {
        let fx = Fx::new();
        let a = fx.add("a@x.co", "rt-a");
        let k = fx.add_api_key(API_KEY);
        fx.switch_to(&k, false).unwrap();
        let (svc, acct) = fx.live_item(ItemKind::ManagedKey);
        fx.kc.delete(&svc, &acct).unwrap();
        let stray = Fx::credential_json("stray@x.co", "rt-stray")
            .to_string()
            .into_bytes();
        fx.set_live_credential(&stray);
        fx.switch_to(&a, force).unwrap();
        let saved = with_bytes(&fx);
        assert_eq!(saved.len(), 1, "force={force}");
        let (entry, bytes) = &saved[0];
        assert_eq!(bytes, &stray, "force={force}");
        assert_eq!(
            (entry.identity.as_ref(), entry.account),
            (None, None),
            "force={force}"
        );
        assert_eq!(entry.reason.as_deref(), Some(reason));
    }
}
