//! §13.6 `doctor`, engine side: one fixture state per check, and the read-only invariant
//! (B.67): doctor creates nothing, changes no byte, and never unlocks or asks.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::*;
use tagteam_cc::live::Platform;
use tagteam_core::{AccountId, ProviderId, WindowKind};
use tagteam_engine::doctor::{DoctorOptions, DoctorReport};
use tagteam_engine::store::{DisplacedRow, Store};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::liveness::FakeProcess;
use tagteam_provider::{Check, CheckStatus, Keychain, ProcessStamp, Provider};

fn doctor(fx: &Fx) -> DoctorReport {
    fx.engine.doctor(DoctorOptions::default()).unwrap()
}

/// Every finding with `id`, in report order.
fn found<'r>(r: &'r DoctorReport, id: &str) -> Vec<&'r Check> {
    r.checks
        .iter()
        .filter(|(_, c)| c.id == id)
        .map(|(_, c)| c)
        .collect()
}

/// The one finding with `id`.
fn one<'r>(r: &'r DoctorReport, id: &str) -> &'r Check {
    let all = found(r, id);
    assert_eq!(all.len(), 1, "{id}: {:#?}", r.checks);
    all[0]
}

/// The one finding with `id` and the provider it was reported under.
fn under<'r>(r: &'r DoctorReport, id: &str) -> (Option<&'r ProviderId>, &'r Check) {
    let all: Vec<_> = r.checks.iter().filter(|(_, c)| c.id == id).collect();
    assert_eq!(all.len(), 1, "{id}: {:#?}", r.checks);
    (all[0].0.as_ref(), &all[0].1)
}

fn fix(c: &Check) -> &str {
    c.fix
        .as_deref()
        .unwrap_or_else(|| panic!("{} names no fix", c.id))
}

fn store_path(fx: &Fx) -> PathBuf {
    fx.env.data_dir().join("tagteam.db")
}

/// A write connection beside the fixture's, for rows no store method writes.
fn sql(fx: &Fx) -> rusqlite::Connection {
    rusqlite::Connection::open(store_path(fx)).unwrap()
}

fn now_s(fx: &Fx) -> i64 {
    use tagteam_provider::Clock;
    fx.clock.now_ms() / 1000
}

/// Every entry under `root`: its mode, and a file's bytes or a link's target. Directories and
/// links are never followed, so the walk itself reads nothing it should not. A store's `-shm`
/// file is listed without its bytes: it is SQLite's shared-memory WAL index, where every reader
/// of a store another process holds open records its read mark, and it holds no row (Decision 3).
fn tree(root: &Path) -> Tree {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>) {
        let Ok(listing) = fs::read_dir(dir) else {
            return;
        };
        for entry in listing {
            let path = entry.unwrap().path();
            let m = fs::symlink_metadata(&path).unwrap();
            let mode = m.permissions().mode();
            if m.file_type().is_symlink() {
                let target = fs::read_link(&path).unwrap();
                out.insert(
                    path,
                    (mode, Some(target.into_os_string().into_encoded_bytes())),
                );
            } else if m.is_dir() {
                out.insert(path.clone(), (mode, None));
                walk(&path, out);
            } else if path.to_string_lossy().ends_with("-shm") {
                out.insert(path, (mode, None));
            } else {
                out.insert(path.clone(), (mode, Some(fs::read(&path).unwrap())));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out);
    out
}

type Tree = BTreeMap<PathBuf, (u32, Option<Vec<u8>>)>;

/// The paths `after` adds, drops or changes against `before`, so a failure names them.
fn changed(before: &Tree, after: &Tree) -> Vec<PathBuf> {
    let paths: std::collections::BTreeSet<&PathBuf> = before.keys().chain(after.keys()).collect();
    paths
        .into_iter()
        .filter(|p| before.get(*p) != after.get(*p))
        .cloned()
        .collect()
}

// ---- Part C: tagteam's own checks and the accounts ----

#[test]
fn tagteam_s_own_findings_come_first_then_each_provider_s() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let r = doctor(&fx);
    let first_provider = r.checks.iter().position(|(p, _)| p.is_some()).unwrap();
    assert!(
        r.checks[first_provider..].iter().all(|(p, _)| p.is_some()),
        "{:#?}",
        r.checks
    );
    assert!(r.checks[..first_provider].iter().all(|(p, _)| p.is_none()));
    assert_eq!(under(&r, "accounts.vault").0, Some(&cc()));
    assert_eq!(under(&r, "store.integrity").0, None);
}

#[test]
fn an_unknown_provider_is_refused_and_a_named_one_narrows_the_report() {
    let ff = FakeFx::new();
    ff.fx.add("a@x.co", "rt-a");
    ff.fake_add("alice", "tok-1", "renew-1");
    let all = ff.engine.doctor(DoctorOptions::default()).unwrap();
    let providers: std::collections::BTreeSet<_> =
        all.checks.iter().filter_map(|(p, _)| p.clone()).collect();
    assert_eq!(
        providers,
        [cc(), ff.fake_provider()].into_iter().collect(),
        "every provider with an account"
    );
    let fake = ff
        .engine
        .doctor(DoctorOptions {
            provider: Some(ff.fake_provider()),
            ..DoctorOptions::default()
        })
        .unwrap();
    assert!(
        fake.checks
            .iter()
            .all(|(p, _)| p.is_none() || p.as_ref() == Some(&ff.fake_provider())),
        "{:#?}",
        fake.checks
    );
    let err = ff
        .engine
        .doctor(DoctorOptions {
            provider: Some(ProviderId::new("nope")),
            ..DoctorOptions::default()
        })
        .unwrap_err();
    assert_eq!(err.kind(), "unknown-provider");
}

#[test]
fn a_signal_ends_doctor_as_interrupted() {
    let fx = Fx::new();
    fx.engine.cancel().request(2);
    let err = fx.engine.doctor(DoctorOptions::default()).unwrap_err();
    assert_eq!(err.signal(), Some(2));
}

#[test]
fn a_missing_data_directory_is_reported_and_nothing_is_created() {
    let fx = Fx::new();
    let before = tree(fx.dir.path());
    let r = doctor(&fx);
    let c = one(&r, "store.present");
    assert_eq!(c.status, CheckStatus::Info);
    assert!(c.message.contains("no state yet"), "{}", c.message);
    assert_eq!(
        changed(&before, &tree(fx.dir.path())),
        Vec::<PathBuf>::new()
    );
    for dir in [fx.env.data_dir(), fx.env.config_dir(), fx.env.state_dir()] {
        assert!(!dir.exists(), "{}", dir.display());
    }
    assert!(r.ok());
}

#[test]
fn a_sound_store_passes_its_integrity_and_schema_checks() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let r = doctor(&fx);
    assert_eq!(one(&r, "store.integrity").status, CheckStatus::Ok);
    assert_eq!(one(&r, "store.schema").status, CheckStatus::Ok);
    assert!(found(&r, "store.skipped").is_empty());
}

#[test]
fn a_file_that_is_not_a_database_fails_the_integrity_check() {
    let fx = Fx::new();
    fs::create_dir_all(fx.env.data_dir()).unwrap();
    fs::write(store_path(&fx), vec![0x5a; 4096]).unwrap();
    let r = doctor(&fx);
    let c = one(&r, "store.integrity");
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(fix(c).contains("backup"), "{c:?}");
    assert_eq!(one(&r, "store.skipped").status, CheckStatus::Info);
    assert!(!r.ok());
}

#[test]
fn a_newer_schema_fails_and_an_older_one_is_information() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    sql(&fx).pragma_update(None, "user_version", 99).unwrap();
    let r = doctor(&fx);
    let c = one(&r, "store.schema");
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(c.message.contains("v99"), "{}", c.message);
    assert!(fix(c).contains("upgrade tagteam"));
    assert!(
        found(&r, "accounts.vault").is_empty(),
        "no row of a store of another schema is read"
    );

    sql(&fx).pragma_update(None, "user_version", 1).unwrap();
    let r = doctor(&fx);
    let c = one(&r, "store.schema");
    assert_eq!(c.status, CheckStatus::Info);
    assert!(c.message.contains("migrates"), "{}", c.message);
    let version: i64 = sql(&fx)
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1, "doctor never migrates (Decision 3)");
}

#[test]
fn open_modes_name_the_chmod_that_fixes_them() {
    let fx = Fx::with_platform(Platform::Linux);
    fx.add("a@x.co", "rt-a");
    let db = store_path(&fx);
    fs::set_permissions(&db, fs::Permissions::from_mode(0o644)).unwrap();
    let data = fx.env.data_dir();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o755)).unwrap();
    let vault_file = data
        .join("vault")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::set_permissions(&vault_file, fs::Permissions::from_mode(0o640)).unwrap();
    let r = doctor(&fx);
    let modes = found(&r, "store.mode");
    let fixes: Vec<&str> = modes.iter().map(|c| fix(c)).collect();
    assert!(modes.iter().all(|c| c.status == CheckStatus::Warn));
    for want in [
        format!("chmod 600 '{}'", db.display()),
        format!("chmod 700 '{}'", data.display()),
        format!("chmod 600 '{}'", vault_file.display()),
    ] {
        assert!(fixes.contains(&want.as_str()), "{want} in {fixes:?}");
    }
}

#[test]
fn a_temp_file_whose_writer_is_gone_warns_and_one_being_written_does_not() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let ours = fx.env.data_dir().join(".tagteam.db.tagteam-4242-0a1b2c3d");
    let beside_cc = fx
        .paths()
        .credentials_file
        .with_file_name(".credentials.json.tagteam-4243-0a1b2c3d");
    let in_flight = fx.env.data_dir().join(".x.json.tagteam-4244-0a1b2c3d");
    for p in [&ours, &beside_cc, &in_flight] {
        fs::write(p, "secret").unwrap();
    }
    fx.process.set(
        4244,
        FakeProcess {
            exists: Some(true),
            ..FakeProcess::default()
        },
    );
    let r = doctor(&fx);
    let temps = found(&r, "store.temp-files");
    let fixes: Vec<String> = temps.iter().map(|c| fix(c).to_owned()).collect();
    assert_eq!(
        fixes,
        [
            format!("rm '{}'", beside_cc.display()),
            format!("rm '{}'", ours.display()),
        ],
        "sorted by path; the live writer's file is left out"
    );
    assert!(
        temps
            .iter()
            .all(|c| c.status == CheckStatus::Warn && c.message.contains("may hold a secret"))
    );
}

#[test]
fn a_temp_file_whose_writer_cannot_be_told_warns_and_is_not_called_clear() {
    // §13.6: an input that cannot be read warns. Codex pre-merge slice 6: the file was skipped
    // and, with nothing else found, the check said no write left a temp file behind.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let unsure = fx.env.data_dir().join(".x.json.tagteam-4245-0a1b2c3d");
    fs::write(&unsure, "secret").unwrap();
    fx.process.set(
        4245,
        FakeProcess {
            exists: None,
            ..FakeProcess::default()
        },
    );
    let r = doctor(&fx);
    let temps = found(&r, "store.temp-files");
    assert_eq!(temps.len(), 1, "{temps:?}");
    let c = temps[0];
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(
        c.message.contains(&unsure.display().to_string())
            && c.message.contains("pid 4245")
            && c.message.contains("cannot be told"),
        "{}",
        c.message
    );
    assert!(
        !fix(c).starts_with("rm ") && fix(c).contains("pid 4245"),
        "{}",
        fix(c)
    );
    assert!(unsure.exists(), "doctor stays read-only");
}

#[test]
fn an_unparseable_settings_file_fails_and_a_bad_value_or_unknown_key_warns() {
    let fx = Fx::new();
    let path = fx.env.config_dir().join("config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "[autoswitch\n").unwrap();
    let r = doctor(&fx);
    let c = one(&r, "settings.file");
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(c.message.contains("not valid TOML"), "{}", c.message);

    fs::write(&path, "bogus = 1\n[autoswitch]\nthreshold = 5\n").unwrap();
    let r = doctor(&fx);
    assert_eq!(one(&r, "settings.file").status, CheckStatus::Ok);
    let value = one(&r, "settings.value");
    assert_eq!(value.status, CheckStatus::Warn);
    assert!(value.message.contains("threshold"), "{}", value.message);
    let unknown = one(&r, "settings.unknown");
    assert_eq!(unknown.status, CheckStatus::Warn);
    assert!(unknown.message.contains("bogus") && fix(unknown).contains("bogus"));
}

#[test]
fn the_log_is_reported_with_its_size_and_whether_it_can_be_written() {
    let fx = Fx::new();
    let r = doctor(&fx);
    assert!(
        one(&r, "log.file")
            .message
            .contains("nothing is logged yet")
    );
    assert_eq!(one(&r, "log.writable").status, CheckStatus::Ok);
    assert!(
        !fx.env.state_dir().exists(),
        "the log's directory is not created"
    );

    let log = fx.env.log_file();
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, "0123456789").unwrap();
    fs::write(log.with_extension("log.1"), "01234").unwrap();
    let r = doctor(&fx);
    let c = one(&r, "log.file");
    assert_eq!(c.status, CheckStatus::Info);
    assert!(c.message.contains("10 bytes, and 5 more"), "{}", c.message);

    fs::set_permissions(&log, fs::Permissions::from_mode(0o400)).unwrap();
    let r = doctor(&fx);
    let c = one(&r, "log.writable");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(fix(c).contains(&log.display().to_string()), "{c:?}");
}

#[test]
fn a_linux_vault_file_naming_no_account_warns() {
    let fx = Fx::with_platform(Platform::Linux);
    fx.add("a@x.co", "rt-a");
    let stray = fx.env.data_dir().join("vault/0000-stray.json");
    fs::write(&stray, "{}").unwrap();
    let r = doctor(&fx);
    let (provider, c) = under(&r, "accounts.orphans");
    assert_eq!(provider, None);
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        fix(c).starts_with(&format!("rm '{}'", stray.display())),
        "{c:?}"
    );
}

#[test]
fn keychain_items_of_the_service_with_no_account_in_the_store_may_be_another_data_dir_s() {
    let fx = Fx::new();
    fx.kc.put(SERVICE, "0192-someone-else", b"{}");
    fx.kc.set_locked(true);
    let r = doctor(&fx);
    let c = one(&r, "accounts.orphans");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.message.contains("another tagteam data directory"),
        "{}",
        c.message
    );
    assert!(fix(c).contains("tagteam purge --keychain-orphans"), "{c:?}");
    assert_eq!(
        fx.kc.unlock_attempts(),
        0,
        "the probe reads attributes only"
    );

    fx.kc.set_locked(false);
    fx.add("a@x.co", "rt-a");
    assert!(
        found(&doctor(&fx), "accounts.orphans").is_empty(),
        "an account in the store makes the service's items its own"
    );
}

#[test]
fn an_account_without_a_vault_entry_fails_naming_remove() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    fx.kc.delete(SERVICE, id.as_str()).unwrap();
    let r = doctor(&fx);
    let (provider, c) = under(&r, "accounts.vault");
    assert_eq!(provider, Some(&cc()));
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(fix(c).contains("tagteam remove 1"), "{c:?}");
    assert!(!r.ok());
}

#[test]
fn an_unreadable_vault_entry_fails() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    fx.kc.set_unreadable(SERVICE, id.as_str(), true);
    let r = doctor(&fx);
    let c = one(&r, "accounts.vault");
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(c.message.contains("cannot be read"), "{}", c.message);
}

#[test]
fn a_locked_keychain_skips_what_reads_it_with_one_warning_and_is_never_unlocked() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    fx.kc.delete(SERVICE, id.as_str()).unwrap();
    fx.kc.set_locked(true);
    let r = doctor(&fx);
    assert!(
        found(&r, "accounts.vault").is_empty(),
        "a vault that cannot be read is not judged: {:#?}",
        r.checks
    );
    let (provider, c) = under(&r, "keychain.locked");
    assert_eq!(provider, None);
    assert_eq!(c.status, CheckStatus::Warn);
    assert_eq!(
        c.message,
        "the login keychain is locked (common over SSH), so the checks that read it were skipped: vault entries, pending replacements, interrupted switches and session credentials",
        "one Keychain, locked: the wording of a release run"
    );
    assert!(fix(c).contains("security unlock-keychain"));
    assert_eq!(fx.kc.unlock_attempts(), 0, "doctor asks nothing (B.67)");
}

#[test]
fn a_quarantined_account_warns_with_its_reason_and_the_relogin_fix() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    fx.quarantine(&id, "invalid_grant", &vault_fp(&fx, &id));
    let r = doctor(&fx);
    let c = one(&r, "accounts.quarantined");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains("invalid_grant"), "{}", c.message);
    assert_eq!(
        fix(c),
        "log in as that account with `claude`, then `tagteam add --position 1`"
    );
    assert!(
        !c.message.contains("a@x.co"),
        "doctor names no email (§4.4)"
    );
}

#[test]
fn a_login_within_seven_days_of_its_expiry_warns() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let store = fx.engine.store().unwrap();
    store
        .set_login_expires_at(&id, Some(now_s(&fx) * 1000 + 2 * 86_400_000))
        .unwrap();
    let c = one(&doctor(&fx), "accounts.login-expiry").clone();
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains("expires in 2d00h"), "{}", c.message);
    store
        .set_login_expires_at(&id, Some(now_s(&fx) * 1000 + 30 * 86_400_000))
        .unwrap();
    assert!(found(&doctor(&fx), "accounts.login-expiry").is_empty());
}

#[test]
fn a_pending_replacement_warns_and_one_whose_landed_metadata_cannot_be_read_fails() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let old = fx.vault_bytes(&id).unwrap();
    let new = credential("a@x.co", "rt-new");
    fx.begin_replacement(&id, &new, &Fx::oauth_account("a@x.co"), "oauth");
    let r = doctor(&fx);
    let c = one(&r, "accounts.replacement");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(fix(c).starts_with("nothing to do"), "{c:?}");

    sql(&fx)
        .execute(
            "UPDATE accounts SET replacing_meta = 'not json' WHERE id = ?1",
            [id.as_str()],
        )
        .unwrap();
    let r = doctor(&fx);
    let c = one(&r, "accounts.replacement");
    assert_eq!(c.status, CheckStatus::Fail, "{c:?}");
    assert_eq!(fix(c), "`tagteam remove 1`, then add the account again");

    sql(&fx)
        .execute(
            "UPDATE accounts SET replacing_meta = NULL WHERE id = ?1",
            [id.as_str()],
        )
        .unwrap();
    let c = one(&doctor(&fx), "accounts.replacement").clone();
    assert_eq!(
        c.status,
        CheckStatus::Warn,
        "a landed replacement that recorded no details is cleared, never refused (§12.5): {c:?}"
    );

    fx.put_vault(&id, &old);
    let c = one(&doctor(&fx), "accounts.replacement").clone();
    assert_eq!(
        c.status,
        CheckStatus::Warn,
        "it never landed: the next lock holder rolls it back"
    );
    let row = fx.engine.store().unwrap().account(&id).unwrap().unwrap();
    assert!(row.replacing_fp.is_some(), "doctor reconciles nothing");
}

#[test]
fn a_stale_marked_live_store_warns_naming_the_forced_switch() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    fx.replace_login(&id, &credential("a@x.co", "rt-imported"), "oauth");
    let c = one(&doctor(&fx), "accounts.live-stale").clone();
    assert_eq!(c.status, CheckStatus::Warn);
    assert_eq!(
        fix(&c),
        "`tagteam switch 1 --force` activates the new login"
    );
}

// ---- Part C: inputs that cannot be read (§13.6: `warn` and why, never `ok`, never nothing) ----

/// `f`'s result with `path` set to `mode`, the mode restored after: how a test makes an input
/// unreadable. Run as a non-root user, whom a mode binds.
fn with_mode<T>(path: &Path, mode: u32, f: impl FnOnce() -> T) -> T {
    let before = fs::metadata(path).unwrap().permissions();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    let out = f();
    fs::set_permissions(path, before).unwrap();
    out
}

#[test]
fn a_data_directory_that_cannot_be_read_warns_and_is_never_taken_for_no_state() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let above = fx.env.data_dir().parent().unwrap().to_path_buf();
    let r = with_mode(&above, 0o000, || doctor(&fx));
    let c = one(&r, "store.present");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(
        c.message.contains("cannot be read (permission denied)"),
        "{}",
        c.message
    );
    assert_eq!(one(&r, "store.skipped").status, CheckStatus::Info);
}

#[test]
fn a_store_that_cannot_be_opened_warns_naming_the_cause_and_is_no_integrity_failure() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let db = store_path(&fx);
    let r = with_mode(&db, 0o000, || doctor(&fx));
    let c = one(&r, "store.integrity");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(
        c.message.contains("cannot be opened (") && c.message.contains("unable to open"),
        "{}",
        c.message
    );
    assert!(
        !fix(c).contains("backup") && fix(c).contains("`store.mode`"),
        "{c:?}"
    );
    let chmod = format!("chmod 600 '{}'", db.display());
    assert!(
        found(&r, "store.mode").iter().any(|m| fix(m) == chmod),
        "store.mode names the chmod: {:#?}",
        r.checks
    );
    assert_eq!(one(&r, "store.skipped").status, CheckStatus::Info);
}

#[test]
fn account_rows_that_cannot_be_read_warn_with_their_cause() {
    // A row of another provider leaves this provider's accounts readable, but no check that
    // needs every account can run.
    for of_this_provider in [true, false] {
        let fx = Fx::new();
        fx.add("a@x.co", "rt-a");
        let b = fx.add("b@x.co", "rt-b");
        let provider = if of_this_provider {
            "claude-code"
        } else {
            "elsewhere"
        };
        sql(&fx)
            .execute(
                "UPDATE accounts SET provider = ?1, position = 'x' WHERE id = ?2",
                [provider, b.as_str()],
            )
            .unwrap();
        let r = doctor(&fx);
        let c = one(&r, "store.accounts");
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(
            c.message.contains("Invalid column type Text"),
            "{}",
            c.message
        );
        assert!(fix(c).contains("from a backup"), "{c:?}");
        let vault = one(&r, "accounts.vault");
        if of_this_provider {
            assert_eq!(vault.status, CheckStatus::Warn);
            assert!(
                vault.message.contains("Invalid column type Text"),
                "{}",
                vault.message
            );
        } else {
            assert_eq!(vault.status, CheckStatus::Ok, "account 1 still reads");
        }
    }
}

#[test]
fn a_replacement_record_or_an_activation_that_cannot_be_read_warns_with_its_cause() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    fx.begin_replacement(
        &id,
        &credential("a@x.co", "rt-new"),
        &Fx::oauth_account("a@x.co"),
        "oauth",
    );
    let db = sql(&fx);
    db.execute(
        "UPDATE accounts SET replacing_meta = X'00' WHERE id = ?1",
        [id.as_str()],
    )
    .unwrap();
    db.execute(
        "INSERT OR REPLACE INTO active_accounts (provider, account_id, login_epoch) \
         VALUES ('claude-code', ?1, 'x')",
        [id.as_str()],
    )
    .unwrap();
    let r = doctor(&fx);
    let c = one(&r, "accounts.replacement");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.message.contains("Invalid column type Blob"),
        "{}",
        c.message
    );
    let c = one(&r, "accounts.live-stale");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.message.contains("cannot be told") && c.message.contains("Invalid column type Text"),
        "{}",
        c.message
    );
}

#[test]
fn a_vault_directory_that_cannot_be_listed_warns_in_every_check_that_lists_it() {
    let fx = Fx::with_platform(Platform::Linux);
    fx.add("a@x.co", "rt-a");
    let vault = fx.env.data_dir().join("vault");
    let r = with_mode(&vault, 0o300, || doctor(&fx));
    for id in ["store.mode", "store.temp-files", "accounts.orphans"] {
        let checks = found(&r, id);
        assert!(
            checks.iter().all(|c| c.status != CheckStatus::Ok),
            "{id}: {checks:#?}"
        );
        assert!(
            checks.iter().any(|c| c.status == CheckStatus::Warn
                && c.message.contains("cannot be listed (permission denied)")),
            "{id}: {checks:#?}"
        );
    }
    // Listed, but no entry's mode can be read.
    let r = with_mode(&vault, 0o600, || doctor(&fx));
    let modes = found(&r, "store.mode");
    assert!(
        modes.iter().any(|c| c.message.starts_with("the mode of ")
            && c.message.contains("cannot be read (permission denied)")),
        "{modes:#?}"
    );
}

#[test]
fn a_log_whose_rotation_or_directory_cannot_be_read_warns() {
    let fx = Fx::new();
    let log = fx.env.log_file();
    let dir = log.parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();
    fs::write(&log, "line\n").unwrap();
    let rotation = dir.join("tagteam.log.1");
    std::os::unix::fs::symlink(&rotation, &rotation).unwrap();
    let r = doctor(&fx);
    let warned: Vec<&Check> = found(&r, "log.file")
        .into_iter()
        .filter(|c| c.status == CheckStatus::Warn)
        .collect();
    assert_eq!(warned.len(), 1, "{:#?}", r.checks);
    assert!(
        warned[0].message.contains("tagteam.log.1 cannot be read"),
        "{}",
        warned[0].message
    );

    let r = with_mode(&dir, 0o600, || doctor(&fx));
    let c = one(&r, "log.writable");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(
        c.message.contains("cannot be told") && c.message.contains("permission denied"),
        "{}",
        c.message
    );
}

// ---- Part C: read-only throughout ----

/// A home in every state the checks above report, with the store held open by the fixture's
/// engine, so doctor reads it through its `-shm` (`mode=ro`).
fn populated(fx: &Fx) {
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    fx.quarantine(&a, "invalid_grant", &vault_fp(fx, &a));
    fx.plant_rescue(&b, &vault_fp(fx, &b), &credential("b@x.co", "rt-next"));
    crashed_switch(fx, &a, &b);
    let locks = fx.env.data_dir().join("locks");
    fs::create_dir_all(&locks).unwrap();
    fs::write(
        locks.join("autoswitch-claude-code.lock"),
        "{\"pid\":1,\"start\":1}\n",
    )
    .unwrap();
    fs::write(fx.env.data_dir().join(".x.json.tagteam-4242-0a1b2c3d"), "s").unwrap();
    let config = fx.env.config_dir();
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("config.toml"), "bogus = 1\n").unwrap();
    let log = fx.env.log_file();
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, "line\n").unwrap();
    // Task 10: a profile with a seed, a baseline, a held reservation's file and a credential,
    // and an entry of the source home on no share list.
    let profile = seeded_profile(fx, &b);
    fs::write(profile.join(".tagteam-baseline.json"), "{}").unwrap();
    fs::create_dir_all(profile.join(".tagteam-launch")).unwrap();
    fs::write(profile.join(".tagteam-launch/4242.lock"), "").unwrap();
    fx.set_profile_credential(&profile, &credential("b@x.co", "rt-profile"));
    fs::create_dir(fx.env.home.join(".claude/new-thing")).unwrap();
}

#[test]
fn doctor_over_a_populated_home_changes_no_byte_and_never_unlocks() {
    for locked in [false, true] {
        let fx = Fx::new();
        populated(&fx);
        fx.kc.set_locked(locked);
        let (files, items) = (tree(fx.dir.path()), fx.kc.items());
        doctor(&fx);
        doctor(&fx);
        assert_eq!(
            changed(&files, &tree(fx.dir.path())),
            Vec::<PathBuf>::new(),
            "locked: {locked}"
        );
        assert_eq!(fx.kc.items(), items, "locked: {locked}");
        assert_eq!(fx.kc.unlock_attempts(), 0);
    }
}

#[test]
fn a_store_no_process_has_open_is_read_immutable_and_gains_no_wal_or_shm() {
    let fx = Fx::with_platform(Platform::Linux);
    let db = store_path(&fx);
    {
        let store = Store::open(&db).unwrap();
        add(&store, &cc(), "0192-a", "a@x.co", 1);
    }
    fx.put_vault(
        &AccountId::from_string("0192-a"),
        &credential("a@x.co", "rt-a"),
    );
    let before = tree(fx.dir.path());
    assert!(!db.with_extension("db-shm").exists());
    let r = doctor(&fx);
    assert_eq!(one(&r, "store.integrity").status, CheckStatus::Ok);
    assert_eq!(
        one(&r, "accounts.orphans").status,
        CheckStatus::Ok,
        "the account's row was read"
    );
    assert_eq!(
        changed(&before, &tree(fx.dir.path())),
        Vec::<PathBuf>::new()
    );
}

// ---- Part D: usage, pending storage, interrupted switches and auto-switch ----

#[test]
fn a_backoff_is_information_with_its_error_and_retry_time() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    sql(&fx)
        .execute(
            "INSERT INTO usage_state (account_id, consecutive_failures, last_error, backoff_until) \
             VALUES (?1, 1, 'http-500', ?2)",
            rusqlite::params![id.as_str(), now_s(&fx) + 120],
        )
        .unwrap();
    let c = one(&doctor(&fx), "usage.backoff").clone();
    assert_eq!(c.status, CheckStatus::Info);
    assert!(
        c.message.contains("after http-500") && c.message.contains("in 2m"),
        "{}",
        c.message
    );
}

#[test]
fn stamps_further_ahead_than_8_4_allows_warn_of_a_skewed_clock() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let now = now_s(&fx);
    sql(&fx)
        .execute(
            "INSERT INTO usage_state (account_id, fetched_at, next_poll_at, backoff_until) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id.as_str(), now + 600, now + 3660 + 600, now + 4500 + 600],
        )
        .unwrap();
    let r = doctor(&fx);
    let skew = found(&r, "usage.clock-skew");
    let what: Vec<bool> = ["reading", "poll plan", "backoff"]
        .iter()
        .map(|w| skew.iter().any(|c| c.message.contains(w)))
        .collect();
    assert_eq!(what, [true, true, true], "{skew:#?}");
    assert!(skew.iter().all(|c| fix(c).contains("system clock")));
    assert!(
        found(&r, "usage.backoff").is_empty(),
        "a skewed backoff does not hold"
    );
}

#[test]
fn an_identity_at_its_hourly_budget_warns() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let key = fx
        .engine
        .store()
        .unwrap()
        .account(&id)
        .unwrap()
        .unwrap()
        .identity_key;
    let c = sql(&fx);
    for i in 0..20 {
        c.execute(
            "INSERT INTO usage_requests (provider, identity_key, at) VALUES ('claude-code', ?1, ?2)",
            rusqlite::params![key, now_s(&fx) - 100 - i],
        )
        .unwrap();
    }
    let r = doctor(&fx);
    let c = one(&r, "usage.budget");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains("all 20 usage requests"), "{}", c.message);
}

#[test]
fn a_rescue_path_that_is_not_a_directory_fails() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fs::write(fx.env.data_dir().join("rescue"), "a file").unwrap();
    let r = doctor(&fx);
    let (provider, c) = under(&r, "pending.rescue-dir");
    assert_eq!(provider, None);
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(fix(c).contains("tagteam purge"), "{c:?}");
}

#[test]
fn a_pending_rescue_warns_under_its_provider_and_an_orphan_under_tagteam() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let rescue = fx.plant_rescue(&id, &vault_fp(&fx, &id), &credential("a@x.co", "rt-next"));
    let orphan = fx
        .env
        .data_dir()
        .join("rescue/0192ffff-0000-7000-8000-000000000000-0-0123456789ab.json");
    fs::write(&orphan, "{}").unwrap();
    let r = doctor(&fx);
    let (provider, c) = under(&r, "pending.rescue");
    assert_eq!(provider, Some(&cc()));
    assert!(c.message.contains(&rescue.display().to_string()));
    let (provider, c) = under(&r, "pending.rescue-orphan");
    assert_eq!(provider, None);
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        fix(c).contains(&format!("rm '{}'", orphan.display())),
        "{c:?}"
    );
}

#[test]
fn displaced_entries_and_their_mismatches_are_information() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let dir = fx.env.data_dir().join("displaced");
    fs::create_dir_all(&dir).unwrap();
    let store = fx.engine.store().unwrap();
    for (id, file) in [
        ("1790000000-0123456789ab-aaaaaa", true),
        ("1790000001-0123456789ab-bbbbbb", false),
    ] {
        store
            .insert_displaced(&DisplacedRow {
                id: id.into(),
                provider: cc(),
                at: 1,
                reason: "displaced-live-login".into(),
                fingerprint: String::new(),
                identity: None,
            })
            .unwrap();
        if file {
            fs::write(dir.join(format!("{id}.json")), "x").unwrap();
        }
    }
    fs::write(dir.join("1790000002-0123456789ab-cccccc.json"), "x").unwrap();
    let r = doctor(&fx);
    let (provider, c) = under(&r, "pending.displaced");
    assert_eq!(provider, Some(&cc()));
    assert_eq!(c.status, CheckStatus::Info);
    assert!(c.message.starts_with("2 displaced"), "{}", c.message);
    let c = one(&r, "pending.displaced-unrecorded");
    assert_eq!(
        fix(c),
        "`tagteam displaced --purge 1790000002-0123456789ab-cccccc` deletes it"
    );
    let c = one(&r, "pending.displaced-missing");
    assert_eq!(c.status, CheckStatus::Info);
    assert!(c.message.contains("1790000001-0123456789ab-bbbbbb"));
}

#[test]
fn an_interrupted_switch_warns_when_recoverable_and_fails_when_undecidable() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &a, &b);
    write_target_credential(&fx, &b);
    let c = one(&doctor(&fx), "switch.interrupted").clone();
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert_eq!(
        fix(&c),
        "`tagteam switch 2` recovers it, then makes the switch it was making"
    );

    fx.set_live_credential(&credential("b@x.co", "rt-rotated-since"));
    let c = one(&doctor(&fx), "switch.interrupted").clone();
    assert_eq!(c.status, CheckStatus::Fail, "{c:?}");
    assert!(fix(&c).starts_with("`tagteam switch 2 --force`"), "{c:?}");
    assert!(journal(&fx).is_some(), "doctor recovers nothing (§13.6)");
}

#[test]
fn the_fix_named_for_a_recoverable_switch_recovers_it() {
    // §13.6: every check that finds a problem names the fix, and that fix must work: `switch N`
    // takes `MutationGuard`, so it recovers the row before it switches (§9.6).
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &a, &b);
    write_target_credential(&fx, &b);
    assert_eq!(
        one(&doctor(&fx), "switch.interrupted").status,
        CheckStatus::Warn
    );
    fx.switch_to(&b, false).unwrap();
    assert!(journal(&fx).is_none(), "the switch recovered the row");
    assert_eq!(
        one(&doctor(&fx), "switch.interrupted").status,
        CheckStatus::Ok
    );
}

#[test]
fn a_switch_under_way_is_information() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    let mut row = crash_row(&fx, &a, &b);
    row.holder = ProcessStamp::current().unwrap();
    fx.engine.store().unwrap().insert_journal(&row).unwrap();
    let c = one(&doctor(&fx), "switch.interrupted").clone();
    assert_eq!(c.status, CheckStatus::Info);
    assert!(c.message.contains("under way"), "{}", c.message);
}

#[test]
fn an_interrupted_switch_is_not_judged_while_the_keychain_is_locked() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &a, &b);
    fx.kc.set_locked(true);
    let r = doctor(&fx);
    let c = one(&r, "switch.interrupted");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains("Keychain is locked"), "{}", c.message);
    assert_eq!(one(&r, "keychain.locked").status, CheckStatus::Warn);
    assert_eq!(fx.kc.unlock_attempts(), 0);
}

#[test]
fn an_engine_runs_exactly_when_its_record_matches_a_live_process() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let lock = fx.env.data_dir().join("locks/autoswitch-claude-code.lock");
    assert_eq!(
        one(&doctor(&fx), "auto.engine").message,
        "auto-switch is not running"
    );
    assert!(
        !lock.exists(),
        "doctor never tries, so never creates, the lock"
    );
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let me = ProcessStamp::current().unwrap();
    fs::write(
        &lock,
        format!("{{\"pid\":{},\"start\":{}}}\n", me.pid, me.start),
    )
    .unwrap();
    let c = one(&doctor(&fx), "auto.engine").clone();
    assert_eq!(c.status, CheckStatus::Info);
    assert_eq!(c.message, format!("auto-switch runs as pid {}", me.pid));
    fs::write(
        &lock,
        format!("{{\"pid\":{},\"start\":{}}}\n", me.pid, me.start + 1),
    )
    .unwrap();
    assert_eq!(one(&doctor(&fx), "auto.engine").status, CheckStatus::Ok);
    fs::write(&lock, "garbage").unwrap();
    let c = one(&doctor(&fx), "auto.engine").clone();
    assert_eq!(c.status, CheckStatus::Warn);
    fix(&c);
}

#[test]
fn unhealthy_ticks_warn() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fx.engine
        .store()
        .unwrap()
        .set_unhealthy_ticks(&cc(), 2)
        .unwrap();
    let c = one(&doctor(&fx), "auto.unhealthy").clone();
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains("last 2 auto-switch"), "{}", c.message);
}

#[test]
fn consume_first_for_a_provider_without_a_long_window_warns() {
    let ff = FakeFx::new();
    ff.fake_add("alice", "tok-1", "renew-1");
    let path = ff.fx.env.config_dir().join("config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "[autoswitch]\nstrategy = \"consume-first\"\n").unwrap();
    let r = ff.engine.doctor(DoctorOptions::default()).unwrap();
    let (provider, c) = under(&r, "auto.strategy");
    assert_eq!(
        provider,
        Some(&ff.fake_provider()),
        "Claude Code has a long window"
    );
    assert_eq!(
        fix(c),
        "`tagteam config set provider.fake-agent.autoswitch.strategy best`"
    );
}

#[test]
fn a_model_no_reading_reports_warns_once_some_account_has_a_reading() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let path = fx.env.config_dir().join("config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "[autoswitch]\nmodels = [\"Fable\", \"Opus\"]\n").unwrap();
    assert!(
        found(&doctor(&fx), "auto.models").is_empty(),
        "nothing to judge by without a reading"
    );
    let now = now_s(&fx);
    record_reading(
        &fx.engine,
        &id,
        &[usage_window(
            "scoped:Opus",
            WindowKind::Scoped,
            10.0,
            now + 3600,
        )],
        now - 10,
        now + 300,
    );
    let r = doctor(&fx);
    let c = one(&r, "auto.models");
    assert!(c.message.contains("\"Fable\""), "{}", c.message);
}

#[test]
fn usage_and_auto_switch_state_that_cannot_be_read_warn_with_their_cause() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let path = fx.env.config_dir().join("config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "[autoswitch]\nmodels = [\"Opus\"]\n").unwrap();
    let db = sql(&fx);
    db.execute(
        "INSERT INTO usage_state (account_id, fetched_at) VALUES (?1, 'x')",
        [id.as_str()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO autoswitch_state (provider, unhealthy_ticks) VALUES ('claude-code', 'x')",
        [],
    )
    .unwrap();
    db.execute(
        "ALTER TABLE usage_requests RENAME TO usage_requests_gone",
        [],
    )
    .unwrap();
    let r = doctor(&fx);
    for (id, cause) in [
        ("usage.state", "Invalid column type Text"),
        ("usage.budget", "no such table: usage_requests"),
        ("auto.unhealthy", "Invalid column type Text"),
        ("auto.models", "Invalid column type Text"),
    ] {
        let c = one(&r, id);
        assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
        assert!(c.message.contains(cause), "{id}: {}", c.message);
    }
}

#[test]
fn an_interrupted_switch_the_live_login_cannot_judge_warns_and_an_undecidable_one_fails() {
    // §13.6: a live login that cannot be read is an input that cannot be read, a `warn` naming
    // why; a readable one whose fingerprints cannot decide fails, naming the forced switch.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &a, &b);
    let (svc, acct) = fx.live_item(tagteam_cc::ItemKind::OAuth);
    fx.kc.set_unreadable(&svc, &acct, true);
    let c = one(&doctor(&fx), "switch.interrupted").clone();
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(
        c.message
            .contains("the live login cannot be read to judge it"),
        "{}",
        c.message
    );
    fx.kc.set_unreadable(&svc, &acct, false);
    fx.set_live_credential(&credential("b@x.co", "rt-rotated-since"));
    let c = one(&doctor(&fx), "switch.interrupted").clone();
    assert_eq!(c.status, CheckStatus::Fail, "{c:?}");
    assert!(fix(&c).starts_with("`tagteam switch 2 --force`"), "{c:?}");
}

// ---- fix round 1 ----

#[test]
fn a_store_with_only_one_of_its_wal_files_warns_and_is_not_opened() {
    // R-T9-sidecars: `-wal` alone hides commits from an immutable open, `-shm` alone makes a
    // read-only open create the `-wal`. Doctor opens neither and creates nothing.
    for keep_wal in [true, false] {
        let fx = Fx::new();
        fx.add("a@x.co", "rt-a");
        let db = store_path(&fx);
        let wal = db.with_extension("db-wal");
        let shm = db.with_extension("db-shm");
        assert!(
            wal.exists() && shm.exists(),
            "the fixture's engine holds the store open"
        );
        fs::remove_file(if keep_wal { &shm } else { &wal }).unwrap();
        let before = tree(fx.dir.path());
        let r = doctor(&fx);
        let c = one(&r, "store.integrity");
        assert_eq!(c.status, CheckStatus::Warn, "keep_wal {keep_wal}: {c:?}");
        assert!(c.message.contains("write-ahead log"), "{}", c.message);
        assert!(fix(c).contains("any tagteam command"), "{c:?}");
        assert_eq!(one(&r, "store.skipped").status, CheckStatus::Info);
        assert!(found(&r, "accounts.vault").is_empty());
        assert_eq!(
            changed(&before, &tree(fx.dir.path())),
            Vec::<PathBuf>::new(),
            "keep_wal {keep_wal}"
        );
    }
}

#[test]
fn every_warning_about_an_input_that_could_not_be_read_names_a_fix() {
    // The log itself, with its directory unreadable.
    let fx = Fx::new();
    let log = fx.env.log_file();
    let dir = log.parent().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();
    fs::write(&log, "line\n").unwrap();
    let r = with_mode(&dir, 0o000, || doctor(&fx));
    for c in found(&r, "log.file") {
        assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
        fix(c);
    }

    // The Keychain probe for orphaned items.
    let fx = Fx::new();
    fx.engine.store().unwrap();
    fx.kc.set_unreadable(SERVICE, "x", true);
    let c = one(&doctor(&fx), "accounts.orphans").clone();
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    fix(&c);

    // The displaced listing, with a file where its directory goes.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    fs::write(fx.env.data_dir().join("displaced"), "a file").unwrap();
    let c = one(&doctor(&fx), "pending.displaced").clone();
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    fix(&c);

    // The switch journal and the auto-switch record, unreadable.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    sql(&fx)
        .execute(
            "ALTER TABLE switch_journal RENAME TO switch_journal_gone",
            [],
        )
        .unwrap();
    let c = one(&doctor(&fx), "switch.interrupted").clone();
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    fix(&c);
    let lock = fx.env.data_dir().join("locks/autoswitch-claude-code.lock");
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    fs::write(&lock, "garbage").unwrap();
    let c = one(&doctor(&fx), "auto.engine").clone();
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    fix(&c);
}

#[test]
fn a_keychain_that_nothing_needs_is_never_asked_about_for_a_store_with_no_accounts() {
    let fx = Fx::new();
    fx.engine.store().unwrap();
    fx.kc.set_locked(true);
    let r = doctor(&fx);
    assert!(found(&r, "keychain.locked").is_empty(), "{:#?}", r.checks);
    assert_eq!(fx.kc.unlock_attempts(), 0);
}

#[test]
fn a_log_whose_nearest_existing_ancestor_is_not_a_directory_is_not_writable() {
    let fx = Fx::new();
    let state = fx.env.state_dir();
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(&state, "a file").unwrap();
    let r = doctor(&fx);
    let c = one(&r, "log.writable");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(c.message.contains("not a directory"), "{}", c.message);
    fix(c);
}

#[test]
fn a_log_path_that_is_a_directory_is_not_writable() {
    let fx = Fx::new();
    let log = fx.env.log_file();
    fs::create_dir_all(&log).unwrap();
    let r = doctor(&fx);
    let c = one(&r, "log.writable");
    assert_eq!(c.status, CheckStatus::Warn, "{c:?}");
    assert!(c.message.contains("is a directory"), "{}", c.message);
    fix(c);
}

// ---- Task 10 Part A: session profiles ----

/// `id`'s profile, made by `Fx::make_profile`, with the account's login epoch as its seed's,
/// agreeing with the vault: a profile as a bootstrap leaves it.
fn seeded_profile(fx: &Fx, id: &AccountId) -> PathBuf {
    let dir = fx.make_profile(id);
    let row = fx.engine.store().unwrap().account(id).unwrap().unwrap();
    fx.write_seed(&dir, row.login_epoch, &vault_fp(fx, id));
    dir
}

#[test]
fn a_real_projects_directory_in_a_profile_fails_and_another_split_file_warns() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = seeded_profile(&fx, &id);
    fs::create_dir(dir.join("projects")).unwrap();
    fs::write(dir.join("CLAUDE.md"), "a private copy\n").unwrap();
    let r = doctor(&fx);
    let splits = found(&r, "sessions.split");
    assert_eq!(splits.len(), 2, "{splits:#?}");
    let projects = splits
        .iter()
        .find(|c| c.message.contains("projects"))
        .unwrap();
    assert_eq!(projects.status, CheckStatus::Fail);
    assert!(fix(projects).starts_with("merge "), "{projects:?}");
    let claude_md = splits
        .iter()
        .find(|c| c.message.contains("CLAUDE.md"))
        .unwrap();
    assert_eq!(claude_md.status, CheckStatus::Warn);
}

#[test]
fn a_marker_that_cannot_be_read_warns_naming_the_directory_to_delete() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let dir = fx
        .env
        .data_dir()
        .join("sessions/0192ffff-0000-7000-8000-00000000000a");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(".tagteam-profile.json"), "not json").unwrap();
    let r = doctor(&fx);
    let (provider, c) = under(&r, "sessions.marker");
    assert_eq!(provider, None);
    assert_eq!(c.status, CheckStatus::Warn);
    assert_eq!(
        fix(c),
        format!(
            "move any real `projects/` or `history.jsonl` in '{}' into the default home's, then delete it once no session runs in it",
            dir.display()
        ),
        "history first, never deletion (§12.2)"
    );
}

#[test]
fn a_profile_without_a_store_account_warns_naming_what_to_delete() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let gone = AccountId::from_string("0192ffff-0000-7000-8000-00000000000b");
    let dir = fx.profile_dir(&gone);
    fx.write_marker(&dir, &gone, &fx.env);
    let r = doctor(&fx);
    let (provider, c) = under(&r, "sessions.orphan");
    assert_eq!(provider, Some(&cc()));
    assert!(
        fix(c).starts_with("move any real `projects/` or `history.jsonl` in ")
            && fix(c).contains(&format!("in '{}' into the default home's", dir.display()))
            && fix(c).contains("then delete it once no session runs in it")
            && fix(c).contains("`tagteam purge`"),
        "history first, never deletion (§12.2): {c:?}"
    );
}

/// A live supervisor in `dir`: its `daemon.lock`, and the probe saying its pid runs.
fn live_supervisor(fx: &Fx, dir: &Path, pid: u32) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("daemon.lock"),
        serde_json::json!({"pid": pid, "origin": "transient", "procStart": LSTART}).to_string(),
    )
    .unwrap();
    fx.process.set(
        pid,
        FakeProcess {
            exists: Some(true),
            start_time_s: tagteam_provider::parse_lstart(LSTART),
            ..FakeProcess::default()
        },
    );
}

#[test]
fn a_live_daemon_in_an_orphaned_profile_is_reported_whoever_owns_the_marker() {
    // §13.6: a profile no stored account owns can hold a supervisor too: found for a registered
    // provider's orphan, one of a provider this build lacks, and one with no marker at all.
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let gone = AccountId::from_string("0192ffff-0000-7000-8000-00000000000b");
    let dir = fx.profile_dir(&gone);
    fx.write_marker(&dir, &gone, &fx.env);
    let sessions = fx.env.data_dir().join("sessions");
    let bare = sessions.join("0192ffff-0000-7000-8000-00000000000c");
    let ghost = AccountId::from_string("0192ffff-0000-7000-8000-00000000000d");
    let ghost_dir = fx.profile_dir(&ghost);
    fs::create_dir_all(&ghost_dir).unwrap();
    tagteam_provider::ProfileMarker {
        provider: ProviderId::new("ghost"),
        account_id: ghost.clone(),
        config_dir: ghost_dir.display().to_string(),
        outer: serde_json::json!({}),
    }
    .write(&ghost_dir)
    .unwrap();
    assert!(found(&doctor(&fx), "sessions.daemon").is_empty());
    for (i, d) in [&dir, &bare, &ghost_dir].into_iter().enumerate() {
        live_supervisor(&fx, d, 5000 + i as u32);
    }
    let r = doctor(&fx);
    let daemons = found(&r, "sessions.daemon");
    assert_eq!(daemons.len(), 3, "{daemons:#?}");
    for d in [&dir, &bare, &ghost_dir] {
        let c = daemons
            .iter()
            .find(|c| c.message.contains(&d.display().to_string()))
            .unwrap_or_else(|| panic!("{}: {daemons:#?}", d.display()));
        assert_eq!(c.status, CheckStatus::Info);
        let fix = fix(c);
        assert!(
            fix.contains("claude daemon stop --any")
                && fix.contains(&format!("'{}'", d.display()))
                && fix.contains(&format!("delete '{}'", d.join("daemon.lock").display())),
            "{fix}"
        );
    }
}

#[test]
fn every_other_orphan_also_moves_history_before_deleting() {
    let fx = Fx::new();
    fx.add("a@x.co", "rt-a");
    let sessions = fx.env.data_dir().join("sessions");
    let bare = sessions.join("0192ffff-0000-7000-8000-00000000000c");
    fs::create_dir_all(&bare).unwrap();
    let ghost = AccountId::from_string("0192ffff-0000-7000-8000-00000000000d");
    let ghost_dir = fx.profile_dir(&ghost);
    fs::create_dir_all(&ghost_dir).unwrap();
    tagteam_provider::ProfileMarker {
        provider: ProviderId::new("ghost"),
        account_id: ghost.clone(),
        config_dir: ghost_dir.display().to_string(),
        outer: serde_json::json!({}),
    }
    .write(&ghost_dir)
    .unwrap();
    let r = doctor(&fx);
    let orphans = found(&r, "sessions.orphan");
    assert_eq!(orphans.len(), 2, "{orphans:#?}");
    for (c, dir) in orphans.iter().zip([&bare, &ghost_dir]) {
        let _ = dir;
        assert!(
            fix(c).starts_with("move any real `projects/` or `history.jsonl` in '"),
            "{c:?}"
        );
        assert!(fix(c).contains("then delete it once no session runs in it"));
    }
    for dir in [&bare, &ghost_dir] {
        assert!(
            orphans
                .iter()
                .any(|c| fix(c).contains(&format!("in '{}'", dir.display()))),
            "{dir:?}: {orphans:#?}"
        );
    }
}

#[test]
fn a_locked_keychain_skips_the_profile_credential_and_provenance_and_says_so_once() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&id);
    let row = fx.engine.store().unwrap().account(&id).unwrap().unwrap();
    let seed = fx.cc.fingerprint(&credential("a@x.co", "rt-seed")).unwrap();
    fx.write_seed(&dir, row.login_epoch, seed.as_str());
    fx.set_profile_credential(&dir, &credential("a@x.co", "rt-profile"));
    // Unlocked, this profile is a provenance conflict (the test above); locked, it is not told.
    assert_eq!(
        one(&doctor(&fx), "sessions.provenance").status,
        CheckStatus::Fail
    );
    fx.kc.set_locked(true);
    let r = doctor(&fx);
    assert!(found(&r, "sessions.credential").is_empty(), "{r:#?}");
    assert!(found(&r, "sessions.provenance").is_empty(), "{r:#?}");
    assert_eq!(one(&r, "keychain.locked").status, CheckStatus::Warn);
    assert_eq!(fx.kc.unlock_attempts(), 0);
}

#[test]
fn a_recorded_spelling_that_is_no_longer_canonical_warns() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = seeded_profile(&fx, &id);
    let tagteam_provider::Read::Present(mut marker) = tagteam_provider::ProfileMarker::read(&dir)
    else {
        panic!("no marker")
    };
    marker.config_dir = "/moved/away/sessions/x".into();
    marker.write(&dir).unwrap();
    let c = one(&doctor(&fx), "sessions.spelling").clone();
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains("/moved/away"), "{}", c.message);
}

#[test]
fn a_reservation_held_after_its_tagteam_died_is_information() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = seeded_profile(&fx, &id);
    let lock = dir.join(".tagteam-launch/4242.lock");
    let held = tagteam_provider::FlockGuard::try_lock(&lock)
        .unwrap()
        .unwrap();
    let c = one(&doctor(&fx), "sessions.reservation").clone();
    assert_eq!(c.status, CheckStatus::Info);
    assert!(c.message.contains("(pid 4242) is gone"), "{}", c.message);
    if cfg!(target_os = "linux") {
        assert!(
            c.message
                .contains(&format!("held by pid {}", std::process::id())),
            "{}",
            c.message
        );
    }
    drop(held);
    assert!(found(&doctor(&fx), "sessions.reservation").is_empty());
}

#[test]
fn a_live_background_daemon_supervisor_in_a_profile_is_information_naming_how_to_stop_it() {
    // §13.6: the profile is session-owned until the supervisor in `daemon.lock` stops.
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = seeded_profile(&fx, &id);
    assert!(found(&doctor(&fx), "sessions.daemon").is_empty());
    fs::write(
        dir.join("daemon.lock"),
        serde_json::json!({"pid": 4343, "origin": "transient", "procStart": LSTART}).to_string(),
    )
    .unwrap();
    assert!(
        found(&doctor(&fx), "sessions.daemon").is_empty(),
        "a supervisor that is gone is no finding"
    );
    fx.process.set(
        4343,
        FakeProcess {
            exists: Some(true),
            start_time_s: tagteam_provider::parse_lstart(LSTART),
            ..FakeProcess::default()
        },
    );
    let r = doctor(&fx);
    let c = one(&r, "sessions.daemon");
    assert_eq!(c.status, CheckStatus::Info);
    assert!(
        c.message.contains("session-owned until it stops"),
        "{}",
        c.message
    );
    let fix = c.fix.as_deref().unwrap();
    assert!(
        fix.contains("claude daemon stop --any")
            && fix.contains("CLAUDE_CONFIG_DIR")
            && fix.contains(&format!("set to '{}'", dir.display()))
            && fix.contains(&format!("delete '{}'", dir.join("daemon.lock").display())),
        "{fix}"
    );
    assert!(found(&r, "sessions.state").is_empty());
}

#[test]
fn a_live_session_does_not_hide_the_daemon_from_doctor_and_an_unreadable_lock_is_named() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = seeded_profile(&fx, &id);
    fx.live_record(&dir, 4242, "interactive");
    live_supervisor(&fx, &dir, 4343);
    let r = doctor(&fx);
    let c = one(&r, "sessions.daemon");
    assert_eq!(c.status, CheckStatus::Info);
    assert!(fix(c).contains("claude daemon stop --any"), "{c:?}");

    fs::remove_file(dir.join("sessions/4242.json")).unwrap();
    fs::write(dir.join("daemon.lock"), "{").unwrap();
    let r = doctor(&fx);
    let c = one(&r, "sessions.state");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.message
            .contains(&dir.join("daemon.lock").display().to_string()),
        "the lock is named: {}",
        c.message
    );
}

#[test]
fn a_baseline_awaiting_merge_back_and_a_profile_awaiting_a_bootstrap_are_information() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = seeded_profile(&fx, &id);
    fs::write(dir.join(".tagteam-baseline.json"), "{}").unwrap();
    let row = fx.engine.store().unwrap().account(&id).unwrap().unwrap();
    fx.write_seed(&dir, row.login_epoch + 1, &vault_fp(&fx, &id));
    let r = doctor(&fx);
    assert_eq!(one(&r, "sessions.baseline").status, CheckStatus::Info);
    let c = one(&r, "sessions.bootstrap");
    assert_eq!(c.status, CheckStatus::Info);
    assert!(c.message.contains("stale-marked"), "{}", c.message);
}

#[test]
fn a_provenance_conflict_fails_naming_the_account_and_the_explicit_replacement() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = fx.make_profile(&id);
    let row = fx.engine.store().unwrap().account(&id).unwrap().unwrap();
    let seed = fx.cc.fingerprint(&credential("a@x.co", "rt-seed")).unwrap();
    fx.write_seed(&dir, row.login_epoch, seed.as_str());
    fx.set_profile_credential(&dir, &credential("a@x.co", "rt-profile"));
    let r = doctor(&fx);
    let (provider, c) = under(&r, "sessions.provenance");
    assert_eq!(provider, Some(&cc()));
    assert_eq!(c.status, CheckStatus::Fail);
    assert_eq!(
        fix(c),
        "log in as account 1 with `claude`, then `tagteam add --position 1`: an explicit replacement settles it"
    );
    assert!(!r.ok());
    assert_eq!(
        tagteam_provider::Seed::read(&dir)
            .present()
            .unwrap()
            .seed_fp,
        seed.as_str(),
        "doctor never applies provenance (§13.6)"
    );
}

#[test]
fn a_profile_credential_that_cannot_be_read_warns() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = seeded_profile(&fx, &id);
    let (svc, acct) = fx.profile_item(&dir);
    fx.kc.put(&svc, &acct, &credential("a@x.co", "rt-a"));
    fx.kc.set_unreadable(&svc, &acct, true);
    let c = one(&doctor(&fx), "sessions.credential").clone();
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.message.contains("cannot switch or launch"),
        "{}",
        c.message
    );
}

#[test]
fn entries_of_the_source_home_on_no_share_list_warn_and_private_ones_do_not() {
    let fx = Fx::new();
    let r = doctor(&fx);
    assert_eq!(one(&r, "sessions.unknown-entries").status, CheckStatus::Ok);
    let claude = fx.env.home.join(".claude");
    fs::create_dir(claude.join("new-thing")).unwrap();
    fs::create_dir(claude.join("cache")).unwrap();
    let c = one(&doctor(&fx), "sessions.unknown-entries").clone();
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.ends_with(": new-thing"), "{}", c.message);
}

#[test]
fn a_sessions_directory_that_cannot_be_listed_warns_once() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    seeded_profile(&fx, &id);
    let sessions = fx.env.data_dir().join("sessions");
    let r = with_mode(&sessions, 0o300, || doctor(&fx));
    let (provider, c) = under(&r, "sessions.profiles");
    assert_eq!(provider, None);
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.message.contains("cannot be listed (permission denied)"),
        "{}",
        c.message
    );
}

#[test]
fn a_session_state_that_cannot_be_read_warns_and_leaves_the_profile_to_its_session() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = seeded_profile(&fx, &id);
    fs::write(dir.join(".tagteam-baseline.json"), "{}").unwrap();
    fs::create_dir_all(dir.join("sessions")).unwrap();
    fs::write(dir.join("sessions/4242.json"), "not json").unwrap();
    let r = doctor(&fx);
    let c = one(&r, "sessions.state");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.message.contains("4242.json") && c.message.contains("counts as in a session"),
        "{}",
        c.message
    );
    assert!(
        found(&r, "sessions.baseline").is_empty(),
        "a profile not known to be quiescent is its session's"
    );
}

#[test]
fn a_profile_identity_that_cannot_be_read_leaves_provenance_untold_with_a_warning() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = seeded_profile(&fx, &id);
    fx.set_profile_credential(&dir, &credential("a@x.co", "rt-profile"));
    fs::write(dir.join(".claude.json"), "{\"oauthAccount\": ").unwrap();
    let c = one(&doctor(&fx), "sessions.provenance").clone();
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains("cannot be told"), "{}", c.message);
}

#[test]
fn a_split_whose_paths_cannot_be_read_warns_that_it_cannot_be_told() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = seeded_profile(&fx, &id);
    // The source's CLAUDE.md, and the profile's own `projects` link, each lead through a
    // directory this user cannot search; the source's `projects` is the fixture's.
    let blocked = fx.dir.path().join("blocked");
    fs::create_dir_all(blocked.join("x")).unwrap();
    let claude = fx.env.home.join(".claude");
    fs::remove_file(claude.join("CLAUDE.md")).unwrap();
    std::os::unix::fs::symlink(blocked.join("x/CLAUDE.md"), claude.join("CLAUDE.md")).unwrap();
    std::os::unix::fs::symlink(blocked.join("x/projects"), dir.join("projects")).unwrap();
    let r = with_mode(&blocked, 0o000, || doctor(&fx));
    let splits = found(&r, "sessions.split");
    for name in ["CLAUDE.md", "projects"] {
        assert!(
            splits.iter().any(|c| c.status == CheckStatus::Warn
                && c.message.contains("cannot be told")
                && c.message
                    .contains(&format!("{name} cannot be read (permission denied)"))),
            "{name}: {splits:#?}"
        );
    }
}

#[test]
fn a_source_home_that_cannot_be_listed_warns() {
    let fx = Fx::new();
    let claude = fx.env.home.join(".claude");
    let r = with_mode(&claude, 0o300, || doctor(&fx));
    let c = one(&r, "sessions.unknown-entries");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.message.contains("cannot be listed (permission denied)"),
        "{}",
        c.message
    );
}

#[test]
fn a_profile_naming_no_identity_still_reports_a_provenance_conflict() {
    // §12.5: an absent identity is no drift. With P, V and S all distinct the table says
    // conflict, which fails; where it says the profile rotated, nothing names the rotation the
    // account's, which warns (Decision 9).
    for (seed_rt, status) in [("rt-seed", CheckStatus::Fail), ("rt-a", CheckStatus::Warn)] {
        let fx = Fx::new();
        let id = fx.add("a@x.co", "rt-a");
        let dir = fx.make_profile(&id);
        let row = fx.engine.store().unwrap().account(&id).unwrap().unwrap();
        let seed = fx.cc.fingerprint(&credential("a@x.co", seed_rt)).unwrap();
        fx.write_seed(&dir, row.login_epoch, seed.as_str());
        fx.set_profile_credential(&dir, &credential("a@x.co", "rt-profile"));
        fs::write(dir.join(".claude.json"), "{}").unwrap();
        let c = one(&doctor(&fx), "sessions.provenance").clone();
        assert_eq!(c.status, status, "{seed_rt}: {c:?}");
    }
}

#[test]
fn an_account_s_profile_without_a_marker_warns_and_its_splits_are_still_judged() {
    // §13.6: a profile is its account's by its directory, whatever its marker says.
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let dir = fx.profile_dir(&id);
    fs::create_dir_all(dir.join("projects")).unwrap();
    let row = fx.engine.store().unwrap().account(&id).unwrap().unwrap();
    fx.write_seed(&dir, row.login_epoch, &vault_fp(&fx, &id));
    let r = doctor(&fx);
    let (provider, c) = under(&r, "sessions.marker");
    assert_eq!(provider, Some(&cc()));
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(
        c.message.contains("account 1's profile marker is missing"),
        "{}",
        c.message
    );
    assert!(
        fix(c).starts_with("move any real `projects/`")
            && fix(c).ends_with("log in again and run `tagteam add --position 1`"),
        "never deletion first: {c:?}"
    );
    let split = one(&r, "sessions.split");
    assert_eq!(split.status, CheckStatus::Fail);
    assert!(split.message.contains("projects"), "{}", split.message);
    assert!(found(&r, "sessions.orphan").is_empty());
}

#[test]
fn an_orphan_of_a_registered_provider_with_no_account_is_found_from_sessions_itself() {
    // §13.6: orphans come from `sessions/`, not from the providers that have accounts.
    let ff = FakeFx::new();
    ff.fx.add("a@x.co", "rt-a");
    let dir = ff
        .fx
        .make_profile_for(ff.fake.as_ref(), &AccountId::from_string("0192-fake"));
    let r = ff.engine.doctor(DoctorOptions::default()).unwrap();
    let (provider, c) = under(&r, "sessions.orphan");
    assert_eq!(provider, None);
    assert!(
        c.message.contains(&dir.display().to_string()) && c.message.contains("fake-agent"),
        "{}",
        c.message
    );
    assert!(
        fix(c).starts_with("move any real `projects/`") && fix(c).contains("`tagteam purge`"),
        "{c:?}"
    );
}

#[test]
fn an_account_s_profile_whose_marker_names_another_provider_is_still_the_account_s() {
    // §5: `sessions/<id>` is account `<id>`'s profile, whatever its marker says. A marker naming
    // another provider (one with no account here, or one this build lacks) warns, and the
    // checks that need no marker still run: a real `projects/` fails.
    let ff = FakeFx::new();
    let fx = &ff.fx;
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    for (id, provider) in [(&a, "fake-agent"), (&b, "ghost")] {
        let dir = fx.profile_dir(id);
        fs::create_dir_all(dir.join("projects")).unwrap();
        tagteam_provider::ProfileMarker {
            provider: ProviderId::new(provider),
            account_id: id.clone(),
            config_dir: dir.display().to_string(),
            outer: serde_json::json!({}),
        }
        .write(&dir)
        .unwrap();
    }
    let r = ff.engine.doctor(DoctorOptions::default()).unwrap();
    let markers = found(&r, "sessions.marker");
    assert_eq!(markers.len(), 2, "{markers:#?}");
    for (c, provider) in markers.iter().zip(["fake-agent", "ghost"]) {
        assert_eq!(c.status, CheckStatus::Warn);
        assert!(
            c.message.contains(&format!("of {provider} instead")),
            "{}",
            c.message
        );
        assert!(fix(c).starts_with("move any real `projects/`"), "{c:?}");
    }
    let splits = found(&r, "sessions.split");
    assert_eq!(splits.len(), 2, "{splits:#?}");
    assert!(splits.iter().all(|c| c.status == CheckStatus::Fail));
    assert!(found(&r, "sessions.orphan").is_empty());
}

// ---- Task 10 Part C: --online ----

/// An engine whose Claude Code sends to `base`, through `http`.
fn engine_for(
    fx: &Fx,
    base: &str,
    http: std::sync::Arc<dyn tagteam_provider::Http>,
) -> tagteam_engine::Engine {
    use tagteam_cc::ClaudeCode;
    use tagteam_cc::endpoints::Endpoints;
    use tagteam_cc::live::LiveStore;
    let provider = ClaudeCode::with_store(LiveStore::new(fx.kc.clone(), Platform::MacOs))
        .with_endpoints(Endpoints::with_base(base));
    tagteam_engine::Engine::new(tagteam_engine::EngineConfig {
        env: fx.env.clone(),
        registry: tagteam_engine::registry::ProviderRegistry::new()
            .with(std::sync::Arc::new(provider)),
        vault: tagteam_engine::vault::Vault::new(Box::new(
            tagteam_engine::vault::KeychainVault::new(fx.kc.clone()),
        )),
        oracle: fx.oracle.clone(),
        clock: fx.clock.clone(),
        http,
        default_provider: cc(),
        settings: tagteam_engine::settings::Settings::default(),
        process: fx.process.clone(),
        run_shell: tagteam_provider::profile::RunShell::Outside,
        spawner: fx.spawner.clone(),
    })
}

fn online(engine: &tagteam_engine::Engine) -> DoctorReport {
    engine
        .doctor(DoctorOptions {
            online: true,
            ..DoctorOptions::default()
        })
        .unwrap()
}

#[test]
fn online_reaches_a_host_that_answers_and_fails_one_that_refuses_the_connection() {
    let fx = Fx::new();
    let server = tagteam_provider::MockServer::start();
    let http = std::sync::Arc::new(tagteam_engine::net::UreqHttp::direct());
    let r = online(&engine_for(&fx, &server.base_url(), http.clone()));
    let c = one(&r, "online.reach");
    assert_eq!(c.status, CheckStatus::Ok);
    assert!(c.message.contains("(HTTP 404)"), "{}", c.message);
    let sent = server.requests();
    assert_eq!(sent.len(), 1, "one request per host");
    assert!(
        sent[0].headers.iter().all(|(k, _)| k != "authorization"),
        "no credential is sent"
    );

    let r = online(&engine_for(&fx, "http://127.0.0.1:9", http));
    let c = one(&r, "online.reach");
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(fix(c).contains("proxy"), "{c:?}");
}

#[test]
fn an_ambiguous_reply_warns_and_without_online_nothing_is_sent() {
    use tagteam_provider::http::{HttpError, HttpResponse, Method};
    let fx = Fx::new();
    doctor(&fx);
    assert!(
        fx.http.requests().is_empty(),
        "doctor sends nothing without --online"
    );
    fx.http.push(
        Method::Get,
        "https://platform.claude.com/",
        Ok(HttpResponse::json_body(404, &serde_json::json!({}))),
    );
    fx.http.push(
        Method::Get,
        "https://api.anthropic.com/",
        Err(HttpError::Ambiguous("reset after connect".into())),
    );
    let r = online(&fx.engine);
    let reach = found(&r, "online.reach");
    let statuses: Vec<CheckStatus> = reach.iter().map(|c| c.status).collect();
    assert_eq!(statuses, [CheckStatus::Ok, CheckStatus::Warn], "{reach:#?}");
}

// ---- Each store's secret reads are gated on its own Keychain (§13.6) ----

/// A Keychain over a `FakeKeychain` that counts the secret reads it is asked for.
struct ReadCounting {
    inner: std::sync::Arc<tagteam_provider::FakeKeychain>,
    finds: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Keychain for ReadCounting {
    fn find(&self, s: &str, a: &str) -> tagteam_provider::Read<Vec<u8>> {
        self.finds.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.find(s, a)
    }
    fn exists(&self, s: &str, a: &str) -> tagteam_provider::Read<()> {
        self.inner.exists(s, a)
    }
    fn upsert(&self, s: &str, a: &str, d: &[u8]) -> Result<(), tagteam_provider::KeychainError> {
        self.inner.upsert(s, a, d)
    }
    fn delete(&self, s: &str, a: &str) -> Result<(), tagteam_provider::KeychainError> {
        self.inner.delete(s, a)
    }
    fn lock_state(&self) -> tagteam_provider::LockState {
        self.inner.lock_state()
    }
    fn unlock(&self) -> bool {
        self.inner.unlock()
    }
    fn service_has_items(&self, s: &str) -> tagteam_provider::Read<bool> {
        self.inner.service_has_items(s)
    }
}

/// Two Keychains, as a compat run has them (`TAGTEAM_TEST_VAULT_KEYCHAIN`): the vault's is
/// `fx.kc`, Claude Code's own stores are in `cc_kc`. Each counts the secret reads it is asked.
struct Split {
    engine: tagteam_engine::Engine,
    cc_kc: std::sync::Arc<tagteam_provider::FakeKeychain>,
    cc_finds: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    vault_finds: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Split {
    fn new(fx: &Fx) -> Split {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;
        use tagteam_cc::ClaudeCode;
        use tagteam_cc::live::LiveStore;
        let cc_kc = Arc::new(tagteam_provider::FakeKeychain::new());
        let cc_finds = Arc::new(AtomicUsize::new(0));
        let vault_finds = Arc::new(AtomicUsize::new(0));
        let provider = ClaudeCode::with_store(
            LiveStore::new(
                Arc::new(ReadCounting {
                    inner: cc_kc.clone(),
                    finds: cc_finds.clone(),
                }),
                Platform::MacOs,
            )
            .with_retry_delay(std::time::Duration::ZERO),
        );
        let engine = tagteam_engine::Engine::new(tagteam_engine::EngineConfig {
            env: fx.env.clone(),
            registry: tagteam_engine::registry::ProviderRegistry::new().with(Arc::new(provider)),
            vault: tagteam_engine::vault::Vault::new(Box::new(
                tagteam_engine::vault::KeychainVault::new(Arc::new(ReadCounting {
                    inner: fx.kc.clone(),
                    finds: vault_finds.clone(),
                })),
            )),
            oracle: fx.oracle.clone(),
            clock: fx.clock.clone(),
            http: fx.http.clone(),
            default_provider: cc(),
            settings: tagteam_engine::settings::Settings::default(),
            process: fx.process.clone(),
            run_shell: tagteam_provider::profile::RunShell::Outside,
            spawner: fx.spawner.clone(),
        });
        Split {
            engine,
            cc_kc,
            cc_finds,
            vault_finds,
        }
    }

    fn doctor(&self) -> DoctorReport {
        self.engine.doctor(DoctorOptions::default()).unwrap()
    }

    fn finds(counter: &std::sync::atomic::AtomicUsize) -> usize {
        counter.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// A profile that is a provenance conflict when both Keychains can be read: its credential in
/// Claude Code's Keychain and the vault's entry both differ from the seed.
fn conflicting_profile(fx: &Fx, split: &Split, id: &AccountId) -> PathBuf {
    let dir = fx.make_profile(id);
    let row = fx.engine.store().unwrap().account(id).unwrap().unwrap();
    let seed = fx.cc.fingerprint(&credential("a@x.co", "rt-seed")).unwrap();
    fx.write_seed(&dir, row.login_epoch, seed.as_str());
    let (svc, acct) = fx.profile_item(&dir);
    split
        .cc_kc
        .put(&svc, &acct, &credential("a@x.co", "rt-profile"));
    dir
}

#[test]
fn a_locked_provider_keychain_skips_what_reads_it_though_the_vault_s_is_open() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let split = Split::new(&fx);
    let dir = conflicting_profile(&fx, &split, &id);
    assert_eq!(
        one(&split.doctor(), "sessions.provenance").status,
        CheckStatus::Fail,
        "with both Keychains open, the profile is a conflict"
    );

    split.cc_kc.set_locked(true);
    let provider_items = split.cc_kc.items();
    let vault_items = fx.kc.items();
    let before = Split::finds(&split.cc_finds);
    let r = split.doctor();
    assert!(found(&r, "sessions.credential").is_empty(), "{r:#?}");
    assert!(found(&r, "sessions.provenance").is_empty(), "{r:#?}");
    let note = one(&r, "keychain.locked");
    assert_eq!(note.status, CheckStatus::Warn);
    assert_eq!(
        note.message,
        "the login keychain is locked (common over SSH), so the checks that read it were skipped: interrupted switches and session credentials",
        "only what the provider's Keychain holds was skipped; the vault's was read"
    );
    assert_eq!(
        Split::finds(&split.cc_finds),
        before,
        "nothing is read from the locked Keychain"
    );
    assert_eq!(split.cc_kc.unlock_attempts(), 0);
    assert_eq!(split.cc_kc.items(), provider_items);
    assert_eq!(fx.kc.items(), vault_items);
    assert!(dir.exists());
}

#[test]
fn a_locked_provider_keychain_leaves_an_interrupted_switch_unjudged_though_the_vault_s_is_open() {
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    let b = fx.add("b@x.co", "rt-b");
    crashed_switch(&fx, &a, &b);
    let split = Split::new(&fx);
    let (svc, acct) = fx.live_item(tagteam_cc::ItemKind::OAuth);
    split.cc_kc.put(&svc, &acct, &credential("a@x.co", "rt-a"));
    split.cc_kc.set_locked(true);
    let before = Split::finds(&split.cc_finds);
    let r = split.doctor();
    let c = one(&r, "switch.interrupted");
    assert_eq!(c.status, CheckStatus::Warn);
    assert!(c.message.contains("Keychain is locked"), "{}", c.message);
    let note = one(&r, "keychain.locked");
    assert_eq!(note.status, CheckStatus::Warn);
    assert!(
        note.message
            .contains("interrupted switches and session credentials")
            && !note.message.contains("vault entries"),
        "{}",
        note.message
    );
    assert_eq!(Split::finds(&split.cc_finds), before);
    assert_eq!(split.cc_kc.unlock_attempts(), 0);
}

#[test]
fn a_locked_vault_keychain_skips_the_vault_reads_though_the_provider_s_is_open() {
    let fx = Fx::new();
    let id = fx.add("a@x.co", "rt-a");
    let split = Split::new(&fx);
    let dir = conflicting_profile(&fx, &split, &id);
    let (svc, acct) = fx.profile_item(&dir);
    fx.kc.delete(SERVICE, id.as_str()).unwrap();
    fx.kc.set_locked(true);
    let vault_items = fx.kc.items();

    // The provider's own store is read: this credential cannot be, and doctor says so.
    split.cc_kc.set_unreadable(&svc, &acct, true);
    let vault_before = Split::finds(&split.vault_finds);
    let r = split.doctor();
    assert_eq!(
        one(&r, "sessions.credential").status,
        CheckStatus::Warn,
        "{r:#?}"
    );
    assert!(found(&r, "accounts.vault").is_empty(), "{r:#?}");
    let note = one(&r, "keychain.locked");
    assert_eq!(note.status, CheckStatus::Warn);
    assert_eq!(
        note.message,
        "the vault's keychain is locked (common over SSH), so the checks that read it were skipped: vault entries and pending replacements",
        "only the vault's Keychain is at fault, and session credentials were read"
    );

    // Readable, it is read too, but the vault entry it would be compared with is not.
    split.cc_kc.set_unreadable(&svc, &acct, false);
    let provider_before = Split::finds(&split.cc_finds);
    let r = split.doctor();
    assert!(
        Split::finds(&split.cc_finds) > provider_before,
        "the provider's credential was read"
    );
    assert!(found(&r, "sessions.provenance").is_empty(), "{r:#?}");
    assert!(found(&r, "accounts.vault").is_empty(), "{r:#?}");
    assert_eq!(
        Split::finds(&split.vault_finds),
        vault_before,
        "nothing is read from the locked vault Keychain"
    );
    assert_eq!(fx.kc.unlock_attempts(), 0);
    assert_eq!(fx.kc.items(), vault_items);
}
