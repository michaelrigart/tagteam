//! `tagteam statusline` through the binary (§13.5), with Review Focus 5. Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs::{self, File};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use common::{
    cc_profile, cmd, login, now_epoch_s, record_reading, seed_home, std_cmd, two_fresh_accounts,
    usage_window,
};
use serde_json::{Value, json};
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId, WindowKind};
use tagteam_engine::store::{SightingRow, SightingSource, Store};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Env, FileKeychain};

const HOUR: i64 = 3_600;
const WEEK: i64 = 604_800;
const YELLOW_77: &str = "\u{1b}[33m77\u{1b}[0m";

/// `a` at position 1 and `b` at position 2 (live), with one reading of `b` taken `age_s` ago:
/// 5h at 9 % resetting in 2h40m, 7d at 77 % in 3d09h, and Fable at 0 %. Each reset is 30 s past
/// its minute, so a countdown read seconds later prints the same text.
fn managed(age_s: i64) -> (tempfile::TempDir, String, String) {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = two_fresh_accounts(d.path());
    let now = now_epoch_s();
    let (r5, r7) = (
        now + 2 * HOUR + 40 * 60 + 30,
        now + 3 * 86_400 + 9 * HOUR + 30,
    );
    record_reading(
        d.path(),
        &b,
        now - age_s,
        &[
            usage_window("5h", "5h", WindowKind::Short, 9.0, Some(r5), Some(5 * HOUR)),
            usage_window("7d", "7d", WindowKind::Long, 77.0, Some(r7), Some(WEEK)),
            usage_window(
                "scoped:Fable",
                "Fable",
                WindowKind::Scoped,
                0.0,
                Some(r7),
                Some(WEEK),
            ),
        ],
    );
    (d, a, b)
}

fn statusline(root: &Path) -> assert_cmd::Command {
    let mut c = cmd(root);
    c.arg("statusline");
    c
}

fn write_config(root: &Path, text: &str) {
    let dir = Env::for_test(root).config_dir();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), text).unwrap();
}

fn store(root: &Path) -> Store {
    Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
}

/// `(mtime_ns, size)`, the pair that keys `live_identity_cache` (§13.5).
fn stat(path: &Path) -> (i64, i64) {
    let m = fs::metadata(path).unwrap();
    let mtime = m.modified().unwrap().duration_since(UNIX_EPOCH).unwrap();
    (mtime.as_nanos() as i64, m.len() as i64)
}

fn set_mtime(path: &Path, at: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

/// Moves the mtime two seconds on, so a rewrite is visible even on a filesystem whose clock is
/// coarser than the test.
fn bump_mtime(path: &Path) {
    let at = fs::metadata(path).unwrap().modified().unwrap();
    set_mtime(path, at + Duration::from_secs(2));
}

#[test]
fn a_managed_login_prints_its_line_from_the_stored_reading() {
    let (d, _, _) = managed(0);
    statusline(d.path())
        .assert()
        .success()
        .stdout(format!("b · 5h 9% · 7d {YELLOW_77}%\n"))
        .stderr("");
    statusline(d.path())
        .arg("--no-color")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n");
    statusline(d.path())
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n");
}

#[test]
fn an_empty_colour_variable_is_as_good_as_unset() {
    let (d, _, _) = managed(0);
    statusline(d.path())
        .env("NO_COLOR", "")
        .assert()
        .success()
        .stdout(format!("b · 5h 9% · 7d {YELLOW_77}%\n"));
    write_config(d.path(), "[ui]\ncolor = \"never\"\n");
    statusline(d.path())
        .env("FORCE_COLOR", "")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n");
}

#[test]
fn the_format_and_colour_come_from_config_toml() {
    let (d, _, _) = managed(0);
    write_config(
        d.path(),
        "[statusline]\nformat = \"{position} {email} {5h_reset} {7d_reset} {model:fable} {spend} {7d}\"\n\n[ui]\ncolor = \"never\"\n",
    );
    statusline(d.path())
        .assert()
        .success()
        .stdout("2 b@x.co 2h40m 3d09h 0 — 77\n");
    statusline(d.path())
        .env("FORCE_COLOR", "1")
        .assert()
        .success()
        .stdout(format!("2 b@x.co 2h40m 3d09h 0 — {YELLOW_77}\n"));
}

#[test]
fn an_invalid_format_falls_back_to_the_default_line() {
    // §6.4: the invalid value reads as its default, and a status bar has nowhere to show the
    // warning, so stderr stays empty.
    let (d, _, _) = managed(0);
    for format in [
        "{nope}",
        "{5h",
        "{model:}",
        "{model: Fable}",
        "{account} {model:{5h}",
    ] {
        write_config(
            d.path(),
            &format!("[statusline]\nformat = {format:?}\n\n[ui]\ncolor = \"never\"\n"),
        );
        statusline(d.path())
            .assert()
            .success()
            .stdout("b · 5h 9% · 7d 77%\n")
            .stderr("");
    }
}

#[test]
fn a_stale_reading_says_how_old_it_is() {
    let (d, _, _) = managed(20 * 60);
    statusline(d.path())
        .arg("--no-color")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77% · 20m old\n");
    let (d, _, _) = managed(14 * 60);
    statusline(d.path())
        .arg("--no-color")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n");
}

#[test]
fn an_unmanaged_login_prints_its_email_and_no_login_prints_nothing() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    fs::create_dir_all(&env.home).unwrap();
    // No `~/.claude.json` at all.
    statusline(d.path())
        .assert()
        .success()
        .stdout("")
        .stderr("");
    seed_home(&env);
    // Claude Code has run, but nobody is logged in.
    statusline(d.path())
        .assert()
        .success()
        .stdout("")
        .stderr("");
    login(
        &env,
        &FileKeychain::new(d.path().join("keychain")),
        "c@x.co",
        "",
        "rt-c",
    );
    statusline(d.path())
        .assert()
        .success()
        .stdout("c@x.co\n")
        .stderr("");
    assert!(
        !env.data_dir().exists(),
        "statusline never creates the store"
    );
}

#[test]
fn claude_json_missing_garbled_or_rewritten() {
    // Review Focus 5.
    let (d, a, b) = managed(0);
    let root = d.path();
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    let path = env.home.join(".claude.json");
    let shows = |expected: &str| {
        statusline(root)
            .arg("--no-color")
            .assert()
            .success()
            .stdout(expected.to_owned())
            .stderr("");
    };
    let key_of = |id: &str| {
        store(root)
            .account(&AccountId::from_string(id))
            .unwrap()
            .unwrap()
            .identity_key
    };
    let cached = || {
        store(root)
            .live_identity_cache(&ProviderId::new(CLAUDE_CODE))
            .unwrap()
            .unwrap()
    };
    shows("b · 5h 9% · 7d 77%\n");
    assert_eq!(cached().identity_key, Some(key_of(&b)));

    // Rewritten between two runs: the new mtime makes the next run re-parse, and re-key the
    // cache to the file as it is now.
    login(&env, &kc, "a@x.co", "", "rt-a");
    bump_mtime(&path);
    shows("a · 5h —% · 7d —%\n");
    let row = cached();
    assert_eq!(row.identity_key, Some(key_of(&a)));
    assert_eq!((row.mtime_ns, row.size), stat(&path));

    // The cache is what keeps the line within budget: a rewrite that keeps both the size and
    // the mtime is not re-parsed, and any change of mtime is.
    let (mtime, size) = (
        fs::metadata(&path).unwrap().modified().unwrap(),
        stat(&path).1,
    );
    login(&env, &kc, "b@x.co", "", "rt-b");
    assert_eq!(
        stat(&path).1,
        size,
        "a@x.co and b@x.co splice to the same size"
    );
    set_mtime(&path, mtime);
    shows("a · 5h —% · 7d —%\n");
    bump_mtime(&path);
    shows("b · 5h 9% · 7d 77%\n");

    // Garbled: nothing, and still success. Missing: nothing.
    fs::write(&path, "{ \"oauthAccount\": ").unwrap();
    shows("");
    fs::remove_file(&path).unwrap();
    shows("");
}

#[test]
fn no_network_no_lock_check_and_no_settings_warnings() {
    // Nothing is recorded, so both accounts are due for a fetch; the Keychain is locked, so a
    // lock check would refuse; and the settings file is corrupt, so any other command warns.
    let d = tempfile::tempdir().unwrap();
    two_fresh_accounts(d.path());
    fs::write(d.path().join("keychain/LOCKED"), "").unwrap();
    write_config(d.path(), "this is [not toml\n");
    let server = MockServer::start();
    server.on(
        "GET",
        "/api/oauth/usage",
        MockReply::Json {
            status: 200,
            body: json!({}),
        },
    );
    cmd(d.path())
        .env("TAGTEAM_TEST_API_BASE", server.base_url())
        .args(["statusline", "--no-color"])
        .assert()
        .success()
        .stdout("b · 5h —% · 7d —%\n")
        .stderr("");
    assert_eq!(server.requests().len(), 0, "statusline sent a request");
}

#[test]
fn a_large_stdin_is_drained_and_ignored() {
    let (d, _, _) = managed(0);
    statusline(d.path())
        .arg("--no-color")
        .write_stdin(vec![b'x'; 100 * 1024])
        .timeout(Duration::from_secs(10))
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n");
    statusline(d.path())
        .arg("--no-color")
        .write_stdin(r#"{"session_id":"s","model":{"display_name":"Fable"}}"#)
        .timeout(Duration::from_secs(10))
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n");
}

#[test]
fn print_config_prints_the_snippet_and_json_is_refused() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    statusline(d.path())
        .arg("--print-config")
        .assert()
        .success()
        .stdout("{\n  \"statusLine\": {\n    \"type\": \"command\",\n    \"command\": \"tagteam statusline\"\n  }\n}\n")
        .stderr(format!(
            "Add this to {}; tagteam never edits Claude Code's settings.\n",
            d.path().join("home/.claude/settings.json").display()
        ));
    let cases: [&[&str]; 2] = [
        &["statusline", "--json"],
        &["statusline", "--print-config", "--json"],
    ];
    for args in cases {
        let out = cmd(d.path())
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json!({"schemaVersion": 1, "error": {"type": "usage",
                   "message": "statusline prints a line of text; run it without --json"}}),
            "{args:?}"
        );
    }
}

#[test]
fn print_config_and_the_json_refusal_never_wait_for_stdin() {
    // Claude Code's session JSON is drained only for a line that will be printed: a pipe that
    // never closes must not hang `--print-config` or the refused `--json`.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    let cases: [&[&str]; 2] = [&["statusline", "--print-config"], &["statusline", "--json"]];
    for args in cases {
        let mut child = std_cmd(d.path())
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _open = child.stdin.take().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(
            status.is_some(),
            "{args:?} waited for a stdin that never closes"
        );
    }
}

#[test]
fn an_unknown_provider_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    statusline(d.path())
        .args(["--provider", "nope"])
        .assert()
        .code(1)
        .stdout("")
        .stderr("tagteam: unknown provider \"nope\"\n");
}

/// Claude Code's session record for this test process, which is the line's parent, in
/// `config_home`: the ancestor a sighting is keyed by (token report spec §6.3). Returns its
/// `startedAt`.
fn record_for_this_process(config_home: &Path, session_id: &str) -> i64 {
    let dir = config_home.join("sessions");
    fs::create_dir_all(&dir).unwrap();
    let pid = std::process::id();
    let started = now_epoch_s() * 1000 - 60_000;
    let record = json!({"pid": pid, "sessionId": session_id, "cwd": "/w",
                        "startedAt": started, "kind": "interactive"});
    fs::write(dir.join(format!("{pid}.json")), record.to_string()).unwrap();
    started
}

fn sightings(root: &Path) -> Vec<SightingRow> {
    store(root)
        .sightings(&ProviderId::new(CLAUDE_CODE))
        .unwrap()
}

#[test]
fn the_statusline_records_its_session_once_a_minute() {
    // Token report spec §6.3: Claude Code runs the line with its session's environment. The
    // line's first ancestor with a session record, this test process, is the execution, and
    // the session is sighted there from the line's own read on (spec §6.4). The binary has no
    // clock to set, so only what holds at any time is asserted here: one tenure is one
    // sighting, whenever the line runs again. Whether a second look extends it (a minute on) or
    // only reads it (within one) is Task 16's, tested on the fake clock.
    let (d, _, _) = managed(0);
    let root = d.path();
    let started = record_for_this_process(&Env::for_test(root).home.join(".claude"), "s-line");
    // The setup's `add`s logged at info, so the log may exist; the lines must not add to it.
    let log = Env::for_test(root).state_dir().join("tagteam.log");
    let logged = fs::read(&log).ok();
    let line = || {
        statusline(root)
            .arg("--no-color")
            .env("CLAUDE_CODE_SESSION_ID", "s-line")
            .assert()
            .success()
            .stdout("b · 5h 9% · 7d 77%\n")
            .stderr("");
    };
    statusline(root).arg("--no-color").assert().success();
    assert!(
        sightings(root).is_empty(),
        "no session id, nothing recorded"
    );
    line();
    let first = sightings(root);
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(
        (
            first[0].session_id.as_str(),
            first[0].home.as_str(),
            first[0].started_at,
            first[0].source
        ),
        ("s-line", "", started, SightingSource::Statusline)
    );
    assert!(
        first[0].first_at > started,
        "the tenure starts at the line's read, not the execution's start: {first:?}"
    );
    // The same execution, looked at again: still one sighting, keyed as before. Only its end
    // may move, and only if a minute passed, so the end is not compared.
    line();
    let again = sightings(root);
    let key = |s: &SightingRow| {
        (
            s.session_id.clone(),
            s.home.clone(),
            s.started_at,
            s.first_at,
            s.source,
        )
    };
    assert_eq!(
        again.iter().map(key).collect::<Vec<_>>(),
        first.iter().map(key).collect::<Vec<_>>(),
        "a second look adds no sighting"
    );
    assert_eq!(
        fs::read(&log).ok(),
        logged,
        "the line logs at debug at most, so it writes no log"
    );
}

#[test]
fn the_statusline_records_a_session_only_in_a_home_tagteam_knows() {
    // Token report spec §6.3: in a run shell the profile's account; with `CLAUDE_CONFIG_DIR`
    // naming the default home, that home; naming any other directory, nothing.
    let (d, a, _) = managed(0);
    let root = d.path();
    let default = Env::for_test(root).home.join(".claude");
    record_for_this_process(&default, "s-explicit");
    statusline(root)
        .env("CLAUDE_CONFIG_DIR", &default)
        .env("CLAUDE_CODE_SESSION_ID", "s-explicit")
        .assert()
        .success();
    let elsewhere = root.join("elsewhere");
    record_for_this_process(&elsewhere, "s-elsewhere");
    statusline(root)
        .env("CLAUDE_CONFIG_DIR", &elsewhere)
        .env("CLAUDE_CODE_SESSION_ID", "s-elsewhere")
        .assert()
        .success();
    let (profile, shell) = cc_profile(root, &a);
    record_for_this_process(&profile, "s-shell");
    statusline(root)
        .env("CLAUDE_CONFIG_DIR", &shell)
        .env("CLAUDE_CODE_SESSION_ID", "s-shell")
        .assert()
        .success()
        .stderr("");
    let mut seen: Vec<(String, String)> = sightings(root)
        .into_iter()
        .map(|s| (s.session_id, s.home))
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("s-explicit".to_owned(), String::new()),
            ("s-shell".to_owned(), a.clone()),
        ]
    );
}

#[test]
fn the_statusline_output_is_unchanged_by_a_sighting_failure() {
    // Token report spec §6.3: a sighting that cannot be written changes neither the line nor
    // the exit code, and the status bar has nowhere to show why.
    let (d, _, _) = managed(0);
    let root = d.path();
    record_for_this_process(&Env::for_test(root).home.join(".claude"), "s-broken");
    let db = Env::for_test(root).data_dir().join("tagteam.db");
    // A busy store: another process holds the write lock throughout. The line still prints,
    // and its write is skipped. How long it waited is the perf tests' to time, not this one's.
    let busy = rusqlite::Connection::open(&db).unwrap();
    busy.execute_batch("BEGIN IMMEDIATE").unwrap();
    statusline(root)
        .arg("--no-color")
        .env("CLAUDE_CODE_SESSION_ID", "s-broken")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n")
        .stderr("");
    busy.execute_batch("ROLLBACK").unwrap();
    drop(busy);
    assert!(sightings(root).is_empty(), "a busy store skips the write");
    // No table to write to at all.
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE session_sightings")
        .unwrap();
    statusline(root)
        .arg("--no-color")
        .env("CLAUDE_CODE_SESSION_ID", "s-broken")
        .assert()
        .success()
        .stdout("b · 5h 9% · 7d 77%\n")
        .stderr("");
}

#[test]
fn a_sighting_never_creates_the_store() {
    // Like the line itself (§5): with no store there is no account to credit, and nothing is
    // created.
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    seed_home(&env);
    login(
        &env,
        &FileKeychain::new(d.path().join("keychain")),
        "c@x.co",
        "",
        "rt-c",
    );
    record_for_this_process(&env.home.join(".claude"), "s-new");
    statusline(d.path())
        .env("CLAUDE_CODE_SESSION_ID", "s-new")
        .assert()
        .success()
        .stdout("c@x.co\n")
        .stderr("");
    assert!(
        !env.data_dir().exists(),
        "a sighting never creates the store"
    );
}
