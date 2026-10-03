//! §14.2 through the real binary: where the log is and with what modes, what its filter
//! takes, the `statusline` fast path that never opens it, a log that cannot be written, a
//! panic's line, a collector thread's log line under `--debug`, and that no line holds an
//! identity or a secret (B.69). Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::{cmd, expire_vault, login, seed_home, std_cmd, two_fresh_accounts};
use serde_json::{Value, json};
use tagteam_cc::{ItemKind, keychain_account, keychain_service};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::splice::replace_top_level;
use tagteam_provider::{Env, FileKeychain, Keychain};

const TAGTEAM_LOG: &str = "TAGTEAM_LOG";
const API_BASE: &str = "TAGTEAM_TEST_API_BASE";
/// §9.4 step 4's WARN line for a capture nothing could attribute.
const CAPTURED: &str = "captured an unverified live credential";

/// The fixture's default state directory, `~/.local/state/tagteam` (§5).
fn state_dir(root: &Path) -> PathBuf {
    Env::for_test(root).state_dir()
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// The binary with its log file off: a fixture's setup, so only the command under test logs.
fn quiet(root: &Path) -> assert_cmd::Command {
    let mut c = cmd(root);
    c.env(TAGTEAM_LOG, "off");
    c
}

/// `a@x.co` at position 1 and `b@x.co` at 2, `b` live, and then Claude Code rotated b's
/// credential in place. With every endpoint offline nothing attributes the rotation, so
/// `switch 1` captures it into b's vault with a WARN line (§9.4 step 4), which the file
/// records by default and stderr does not, and the oracle's missing answer is a DEBUG line.
fn rotated_live_login(root: &Path) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    quiet(root).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    quiet(root).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b-rotated");
}

/// The log's text, oldest first across its rotations; empty when there is none.
fn log_text(dir: &Path) -> String {
    ["tagteam.log.2", "tagteam.log.1", "tagteam.log"]
        .iter()
        .map(|n| fs::read_to_string(dir.join(n)).unwrap_or_default())
        .collect()
}

/// Decision 8's line: `<UTC time to the millisecond> <pid> <LEVEL> <target>: …`.
fn assert_line_shape(line: &str, level: &str, target: &str) {
    let parts: Vec<&str> = line.splitn(5, ' ').collect();
    assert_eq!(parts.len(), 5, "{line}");
    let time = parts[0].as_bytes();
    assert!(
        time.len() == 24 && time[10] == b'T' && time[19] == b'.' && time[23] == b'Z',
        "{line}"
    );
    assert!(parts[1].parse::<u32>().is_ok(), "{line}");
    assert_eq!((parts[2], parts[3]), (level, format!("{target}:").as_str()));
}

#[test]
fn a_switch_logs_to_a_private_file_in_the_state_directory() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let dir = state_dir(d.path());
    assert!(!dir.exists(), "the setup logged nothing");
    cmd(d.path())
        .args(["switch", "1", "--json"])
        .assert()
        .success()
        .stderr("");
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join("tagteam.log")), 0o600);
    let log = log_text(&dir);
    let line = log
        .lines()
        .find(|l| l.contains(CAPTURED))
        .unwrap_or_else(|| panic!("no capture line in:\n{log}"));
    assert_line_shape(line, "WARN", "tagteam_engine::switch");
    assert!(
        !log.contains("the profile oracle gave no answer"),
        "DEBUG stays out by default:\n{log}"
    );
    let home = d.path().join("home").display().to_string();
    assert!(!log.contains(&home), "paths under HOME are ~/…:\n{log}");
}

#[test]
fn debug_adds_tagteam_s_debug_lines_to_the_file() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    cmd(d.path())
        .args(["switch", "1", "--debug"])
        .assert()
        .success();
    let log = log_text(&state_dir(d.path()));
    let line = log
        .lines()
        .find(|l| l.contains("the profile oracle gave no answer"))
        .unwrap_or_else(|| panic!("no DEBUG line in:\n{log}"));
    assert_line_shape(line, "DEBUG", "tagteam_engine::oracle");
}

#[test]
fn xdg_state_home_moves_the_log() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let state = d.path().join("state");
    cmd(d.path())
        .env("XDG_STATE_HOME", &state)
        .args(["switch", "1"])
        .assert()
        .success();
    assert!(log_text(&state.join("tagteam")).contains(CAPTURED));
    assert!(!state_dir(d.path()).exists());
}

#[test]
fn tagteam_log_off_writes_no_file() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    quiet(d.path()).args(["switch", "1"]).assert().success();
    assert!(!state_dir(d.path()).exists());
}

#[test]
fn tagteam_log_off_leaves_stderr_its_debug_lines() {
    // `off` removes the file layer only: `--debug` still shows its diagnostics on stderr.
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let out = quiet(d.path())
        .args(["switch", "1", "--debug"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("the profile oracle gave no answer"),
        "{stderr}"
    );
    assert!(!state_dir(d.path()).exists());
}

#[test]
fn tagteam_log_replaces_the_whole_file_filter() {
    // Only the switch module, at WARN: nothing else the switch logs reaches the file.
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    cmd(d.path())
        .env(TAGTEAM_LOG, "tagteam_engine::switch=warn")
        .args(["switch", "1"])
        .assert()
        .success();
    let log = log_text(&state_dir(d.path()));
    assert!(log.contains(CAPTURED), "{log}");
    for line in log.lines() {
        assert_eq!(
            line.split(' ').nth(3),
            Some("tagteam_engine::switch:"),
            "{line}"
        );
    }
}

#[test]
fn an_invalid_tagteam_log_warns_once_and_keeps_the_default() {
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let out = cmd(d.path())
        .env(TAGTEAM_LOG, "tagteam=loud")
        .args(["switch", "1", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.starts_with("warning: TAGTEAM_LOG is not a valid filter")
            && stderr.lines().count() == 1,
        "{stderr}"
    );
    // B.36: the warning is on stderr; stdout is still exactly one object.
    serde_json::from_slice::<Value>(&out.stdout).unwrap();
    assert!(log_text(&state_dir(d.path())).contains(CAPTURED));
}

/// Gives `email`'s account a `usage_state` row the store cannot read: text where it keeps an
/// integer.
fn unreadable_usage_state(root: &Path, email: &str) {
    rusqlite::Connection::open(Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO usage_state (account_id, fetched_at) \
             SELECT id, 'soon' FROM accounts WHERE email = ?1",
            [email],
        )
        .unwrap();
}

#[test]
fn statusline_with_the_default_filter_opens_no_log() {
    // §14.2, Review Focus 4: the status bar runs every few seconds and logs at DEBUG at most,
    // so by default it never creates, opens or rotates the log. Nor when the live account's
    // usage cannot be read: an account command's result logs that at WARN, the status bar at
    // DEBUG, and its line still names the account.
    for unreadable in [false, true] {
        let d = tempfile::tempdir().unwrap();
        rotated_live_login(d.path());
        if unreadable {
            unreadable_usage_state(d.path(), "b@x.co");
        }
        let out = cmd(d.path())
            .arg("statusline")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let line = String::from_utf8(out).unwrap();
        assert!(
            line.starts_with("b · "),
            "unreadable usage {unreadable}: {line:?}"
        );
        assert!(
            !state_dir(d.path()).exists(),
            "unreadable usage {unreadable}: {}",
            log_text(&state_dir(d.path()))
        );
    }
}

#[test]
fn a_log_that_cannot_be_written_fails_no_command() {
    // §14.2, §15.2. The state directory is under a regular file, so it can never be made, by
    // root either. Silent by default; `--debug` says so, once.
    for debug in [false, true] {
        let d = tempfile::tempdir().unwrap();
        rotated_live_login(d.path());
        fs::write(d.path().join("blocker"), "").unwrap();
        let mut c = cmd(d.path());
        c.env("XDG_STATE_HOME", d.path().join("blocker/state"))
            .args(["switch", "1", "--json"]);
        if debug {
            c.arg("--debug");
        }
        let out = c.assert().success().get_output().clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap()["switched"],
            true
        );
        let stderr = String::from_utf8(out.stderr).unwrap();
        let notices = stderr.matches("cannot be written").count();
        assert_eq!(notices, usize::from(debug), "{stderr}");
    }
}

#[test]
fn a_panic_is_logged_once_with_its_location_and_stderr_keeps_the_default_hook() {
    // Decision 9: logged at ERROR before the process unwinds, and left out of stderr, where
    // the default hook prints the panic.
    let d = tempfile::tempdir().unwrap();
    rotated_live_login(d.path());
    let out = cmd(d.path())
        .env("TAGTEAM_TEST_FAIL_AT", "panic:after-journal")
        .args(["switch", "1"])
        .assert()
        .code(101)
        .get_output()
        .clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("panicked at") && stderr.contains("injected panic at after-journal"),
        "{stderr}"
    );
    assert!(!stderr.contains("location="), "{stderr}");
    let log = log_text(&state_dir(d.path()));
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| l.contains(" tagteam::panic: "))
        .collect();
    assert_eq!(lines.len(), 1, "{log}");
    assert_line_shape(lines[0], "ERROR", "tagteam::panic");
    assert!(
        lines[0].contains("location=crates/tagteam-engine/src/hooks.rs:"),
        "{}",
        lines[0]
    );
    assert!(
        !lines[0].contains("after-journal"),
        "a formatted message is left to stderr: {}",
        lines[0]
    );
}

#[test]
fn a_collector_thread_s_log_line_under_debug_never_hangs_the_command() {
    // `list` collects each account on a thread of its own (§8.3). a's refresh is refused with
    // invalid_grant, so the gate quarantines a from that thread with a WARN line, which
    // `--debug` prints on stderr. The main thread waits for that thread meanwhile: it must not
    // be holding stderr.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let (a, _b) = two_fresh_accounts(root);
    expire_vault(root, &a, -60_000);
    let server = MockServer::start();
    server.on(
        "POST",
        "/v1/oauth/token",
        MockReply::Json {
            status: 400,
            body: json!({"error": "invalid_grant"}),
        },
    );
    let mut child = std_cmd(root)
        .env(API_BASE, server.base_url())
        .args(["list", "--debug"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("`list --debug` hung");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("quarantined"), "{stderr}");
    assert_eq!(server.hits("POST", "/v1/oauth/token"), 1);
}

// No identity and no secret in any line, at any level (§14.2, §15.3 "Logs", B.69).

const USAGE: &str = "/api/oauth/usage";
const PROFILE: &str = "/api/oauth/profile";
const TOKEN: &str = "/v1/oauth/token";
const ORG_UUID: &str = "org-zq-7735";
const ORG_NAME: &str = "Zqorg Redaction Holdings";
const ALPHA_EMAIL: &str = "zq-alpha-7731@redact.test";
const BRAVO_EMAIL: &str = "zq-bravo-7732@redact.test";
const KEY_EMAIL: &str = "zq-key-7733@redact.test";
const SETUP_EMAIL: &str = "zq-setup-7734@redact.test";
const ALPHA_RT: &str = "zqrt-alpha-Kp7wXr2mQv9sLt4nBy6c";
const ALPHA_AT: &str = "zqat-alpha-Hj3kPw8xRt5vNm2qZs7d";
const ALPHA_RT_NEXT: &str = "zqrt-alpha-next-Pz5wKq8mXr3vTn7y";
const ALPHA_AT_NEXT: &str = "zqat-alpha-next-Lk4xWp9zQm2rVs6t";
const BRAVO_RT: &str = "zqrt-bravo-Wq4zLp9kXv2mRt7nHs3j";
const BRAVO_AT: &str = "zqat-bravo-Tn6yMk3wQp8xVr5zLs2h";
const BRAVO_RT_ROTATED: &str = "zqrt-bravo-rotated-Gx7pKw2zRm9qTv4s";
const BRAVO_AT_ROTATED: &str = "zqat-bravo-rotated-Fy3nVq6kWp8zXm2r";
const API_KEY: &str = "sk-ant-api03-zqkey-Rw8pXk3mQz7vTn2sLy5h";
const SETUP_TOKEN: &str = "sk-ant-oat01-zqsetup-Mx6kPw2zRq9vTs4nLy7j";
/// No line holds one of these whole.
const IDENTITIES: [&str; 6] = [
    ALPHA_EMAIL,
    BRAVO_EMAIL,
    KEY_EMAIL,
    SETUP_EMAIL,
    ORG_NAME,
    ORG_UUID,
];
/// No line holds 13 consecutive characters of one of these.
const SECRETS: [&str; 10] = [
    ALPHA_RT,
    ALPHA_AT,
    ALPHA_RT_NEXT,
    ALPHA_AT_NEXT,
    BRAVO_RT,
    BRAVO_AT,
    BRAVO_RT_ROTATED,
    BRAVO_AT_ROTATED,
    API_KEY,
    SETUP_TOKEN,
];

/// What `claude /login` leaves behind, as `common::login` writes it, but with this fixture's
/// organization and tokens: strings that nothing else in a run could produce.
fn login_as(root: &Path, email: &str, rt: &str, at: &str) {
    let env = Env::for_test(root);
    let path = env.home.join(".claude.json");
    let account = json!({"emailAddress": email, "organizationUuid": ORG_UUID,
                         "organizationName": ORG_NAME, "accountUuid": format!("uuid-{email}")});
    let doc = fs::read(&path).unwrap();
    fs::write(
        &path,
        replace_top_level(&doc, "oauthAccount", &account).unwrap(),
    )
    .unwrap();
    let credential = json!({"claudeAiOauth": {"accessToken": at, "refreshToken": rt,
                            "refreshTokenExpiresAt": 1_797_000_000_000i64}});
    FileKeychain::new(root.join("keychain"))
        .upsert(
            &keychain_service(&env, ItemKind::OAuth),
            &keychain_account(&env),
            credential.to_string().as_bytes(),
        )
        .unwrap();
}

/// The binary at TRACE, with every endpoint on `server`.
fn traced(root: &Path, server: &MockServer) -> assert_cmd::Command {
    let mut c = cmd(root);
    c.env(TAGTEAM_LOG, "trace").env(API_BASE, server.base_url());
    c
}

/// A server for the usage fetches (the recorded reply) and for a refresh, which hands out a's
/// next tokens. The profile oracle answers nothing until `oracle_names_bravo`.
fn redaction_server() -> MockServer {
    let server = MockServer::start();
    let usage: Value = serde_json::from_str(include_str!(
        "../../tagteam-cc/tests/fixtures/endpoints/usage-200.json"
    ))
    .unwrap();
    server.on(
        "GET",
        USAGE,
        MockReply::Json {
            status: 200,
            body: usage["body"].clone(),
        },
    );
    server.on(
        "POST",
        TOKEN,
        MockReply::Json {
            status: 200,
            body: json!({"access_token": ALPHA_AT_NEXT, "refresh_token": ALPHA_RT_NEXT,
                         "expires_in": 28800, "scope": "user:inference user:profile"}),
        },
    );
    server
}

/// From now on the profile oracle names b, with the organization's name, for any token: `add`
/// asks it too, and would refuse a's login as b's.
fn oracle_names_bravo(server: &MockServer) {
    server.on(
        "GET",
        PROFILE,
        MockReply::Json {
            status: 200,
            body: json!({"account": {"uuid": format!("uuid-{BRAVO_EMAIL}"), "email": BRAVO_EMAIL},
                         "organization": {"uuid": ORG_UUID, "name": ORG_NAME}}),
        },
    );
}

#[test]
fn every_command_at_trace_leaves_no_identity_or_secret_in_the_log() {
    // §15.3 "Logs", B.69 and Review Focus 5: every command, at TRACE, against a home whose
    // emails, organization name, tokens and keys are strings nothing else contains, while
    // the tokens go over the wire (usage bearers, the oracle's bearer and its answer, a
    // refresh's body and reply, `add-token`'s key and setup token).
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let server = redaction_server();
    let run = |args: &[&str]| {
        traced(root, &server).args(args).assert().success();
    };
    seed_home(&Env::for_test(root));
    login_as(root, ALPHA_EMAIL, ALPHA_RT, ALPHA_AT);
    run(&["add"]);
    login_as(root, BRAVO_EMAIL, BRAVO_RT, BRAVO_AT);
    run(&["add", "--alias", "zqb"]);
    // Collects both: a with its vault token, b with the live one.
    let listed = traced(root, &server)
        .args(["list", "--json"])
        .output()
        .unwrap();
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let alpha = listed["accounts"][0]["id"].as_str().unwrap().to_owned();
    run(&["list"]);
    run(&["status"]);
    run(&["status", "--json"]);
    run(&["alias", "1", "zqa"]);
    run(&["alias"]);
    run(&["alias", "1", "--unset"]);
    run(&["disable", ALPHA_EMAIL]);
    run(&["enable", ALPHA_EMAIL]);
    run(&["move", "1", "2"]);
    run(&["move", "2", "1"]);
    run(&["history", "1"]);
    run(&["history", "2", "--csv"]);
    run(&["statusline"]);
    // Claude Code rotated b in place, and a's access token is about to expire: `switch 1`
    // asks the oracle about b's new token, captures it, and refreshes a through the gate.
    login_as(root, BRAVO_EMAIL, BRAVO_RT_ROTATED, BRAVO_AT_ROTATED);
    expire_vault(root, &alpha, 60_000);
    oracle_names_bravo(&server);
    run(&["switch", "1"]);
    run(&["switch", "2", "--json"]);
    run(&["add-token", API_KEY, "--email", KEY_EMAIL, "--json"]);
    traced(root, &server)
        .args(["add-token", "-", "--email", SETUP_EMAIL])
        .write_stdin(format!("{SETUP_TOKEN}\n"))
        .assert()
        .success();
    run(&["remove", KEY_EMAIL]);
    run(&["remove", SETUP_EMAIL, "--json"]);
    run(&["config", "path"]);
    run(&["config", "set", "ui.color", "never"]);
    run(&["config", "get", "ui.color"]);
    run(&["config", "list", "--json"]);
    run(&["config", "unset", "ui.color"]);
    run(&["list"]);

    // The secrets were sent: the log is clean because it never writes them.
    let sent = server.requests();
    let bearer = |at: &str| {
        sent.iter().any(|r| {
            r.headers
                .iter()
                .any(|(k, v)| k == "authorization" && *v == format!("Bearer {at}"))
        })
    };
    assert!(
        bearer(ALPHA_AT) && bearer(BRAVO_AT) && bearer(BRAVO_AT_ROTATED),
        "usage and oracle bearers"
    );
    assert!(
        sent.iter()
            .any(|r| r.path == TOKEN && String::from_utf8_lossy(&r.body).contains(ALPHA_RT)),
        "the refresh sent a's refresh token"
    );

    let log = log_text(&state_dir(root));
    assert!(
        log.contains(" INFO tagteam_engine::store: ")
            && log.contains(" INFO tagteam_engine::vault: "),
        "the commands logged their state changes:\n{log}"
    );
    for line in log.lines() {
        let target = line.split(' ').nth(3).unwrap_or_default();
        assert!(
            target.starts_with("tagteam"),
            "only tagteam's own events; no `log` record is bridged in (Decision 5): {line}"
        );
    }
    for identity in IDENTITIES {
        assert!(!log.contains(identity), "{identity} is in the log:\n{log}");
    }
    for secret in SECRETS {
        for part in secret.as_bytes().windows(13) {
            let part = std::str::from_utf8(part).unwrap();
            assert!(
                !log.contains(part),
                "{part:?}, part of a secret, is in the log:\n{log}"
            );
        }
    }
}
