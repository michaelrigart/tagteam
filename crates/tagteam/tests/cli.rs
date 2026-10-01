//! Drives the real binary. Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::os::fd::{FromRawFd, OwnedFd};
use std::path::Path;
use std::process::Stdio;

use assert_cmd::assert::OutputAssertExt;
use common::{LOCKED, cmd, expire_vault, live_email, login, seed_home, std_cmd, two_accounts};
use predicates::prelude::PredicateBooleanExt;
use serde_json::{Value, json};
use tagteam_core::AccountId;
use tagteam_engine::store::Store;
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Env, FileKeychain, Keychain};

/// A stdout whose reader is already gone, so every write to it fails with EPIPE.
fn closed_stdout() -> Stdio {
    let mut fds = [0; 2];
    // SAFETY: `pipe` fills `fds` with two new descriptors on success, and each is owned once.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    drop(read);
    Stdio::from(write)
}

fn assert_home_empty(root: &Path) {
    assert!(
        std::fs::read_dir(root.join("home"))
            .unwrap()
            .next()
            .is_none(),
        "HOME must stay empty"
    );
}

#[test]
fn a_fresh_machine_lists_nothing_and_creates_nothing() {
    // Review Focus 4: no `~/.claude`, no `~/.claude.json`, and nothing created by any of this.
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    let out = cmd(d.path())
        .args(["list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "activeAccountNumber": null, "activeByProvider": {"claude-code": null}, "accounts": []})
    );
    cmd(d.path())
        .args(["status", "--json"])
        .assert()
        .success()
        .stdout("{\"schemaVersion\":1,\"provider\":\"claude-code\",\"active\":null}\n");
    assert_home_empty(d.path());
    cmd(d.path())
        .arg("add")
        .assert()
        .code(1)
        .stderr(predicates::str::contains("no live login"));
    assert_home_empty(d.path());
}

#[test]
fn usage_errors_exit_2_and_keep_the_json_contract() {
    let d = tempfile::tempdir().unwrap();
    cmd(d.path()).args(["move", "1"]).assert().code(2);
    cmd(d.path()).arg("frobnicate").assert().code(2);
    cmd(d.path()).arg("--bogus").assert().code(2);
    let out = cmd(d.path())
        .args(["frobnicate", "--json"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap()["error"]["type"],
        "usage"
    );
}

#[test]
fn a_usage_error_never_echoes_an_argument() {
    // Secrets never reach error output, and a token is an argument like any other, whatever
    // its shape: no usage error repeats an argument's value.
    const SENTINEL: &str = "REVIEW-SENTINEL";
    let d = tempfile::tempdir().unwrap();
    let cases: [&[&str]; 6] = [
        &["add-token", "-", "sk-ant-api03-REVIEW-SENTINEL"],
        &["add-token", "-", "plain-REVIEW-SENTINEL"],
        &["add-token", "tok", "--position", "REVIEW-SENTINEL"],
        &["add-token", "--REVIEW-SENTINEL"],
        &["REVIEW-SENTINEL"],
        &["add-token", "-", "sk-ant-api03-REVIEW-SENTINEL", "--json"],
    ];
    for args in cases {
        let out = cmd(d.path())
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .clone();
        let (stdout, stderr) = (
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(
            !stdout.contains(SENTINEL) && !stderr.contains(SENTINEL),
            "{args:?}:\n{stdout}\n{stderr}"
        );
        if !args.contains(&"--json") {
            assert!(stderr.starts_with("error: "), "{args:?}: {stderr}");
        }
    }
    // The kind and the usage line, which come from the command's own definition, stay.
    cmd(d.path())
        .args(["add-token", "-", "sk-ant-api03-REVIEW-SENTINEL"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(
            "error: unexpected argument found\n\nUsage: tagteam add-token [OPTIONS] [TOKEN]\n\nFor more information, try '--help'.\n",
        );
    // So does a suggestion: it names a real name, never what was typed.
    cmd(d.path())
        .arg("swtich")
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "a similar subcommand exists: 'switch'",
        ));
}

#[test]
fn help_and_version_under_json_are_a_json_usage_error() {
    // They print text, and `--json` promises exactly one JSON object on stdout.
    let d = tempfile::tempdir().unwrap();
    for flag in ["--help", "--version"] {
        let out = cmd(d.path())
            .args([flag, "--json"])
            .assert()
            .code(2)
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json!({"schemaVersion": 1, "error": {"type": "usage",
                   "message": "--help and --version print text; run them without --json"}}),
            "{flag}"
        );
    }
    // Without `--json`, help is the usual page on stdout.
    cmd(d.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::starts_with(
            "Multi-account switcher for AI coding agent CLIs\n",
        ));
}

#[test]
fn a_closed_stdout_is_not_a_panic() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    let cases: [(&[&str], i32); 3] = [
        (&["frobnicate", "--json"], 2),
        (&["list", "--json"], 0),
        (&["list"], 0),
    ];
    for (args, code) in cases {
        std_cmd(d.path())
            .args(args)
            .stdout(closed_stdout())
            .output()
            .unwrap()
            .assert()
            .code(code)
            .stderr(predicates::str::contains("panicked").not());
    }
}

#[test]
fn add_token_reads_a_line_from_stdin() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    cmd(d.path())
        .args(["add-token", "-"])
        .write_stdin("sk-ant-api03-from-stdin\n")
        .assert()
        .success();
    let out = cmd(d.path())
        .args(["list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["accounts"][0]["usageStatus"], "api_key");
}

#[test]
fn a_locked_keychain_without_a_terminal_fails_and_creates_nothing() {
    // Appendix A.3 through the binary: no terminal, so no prompt; `list` and `status` run no check.
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    std::fs::create_dir_all(d.path().join("keychain")).unwrap();
    std::fs::write(d.path().join("keychain/LOCKED"), "").unwrap();
    cmd(d.path()).args(["list", "--json"]).assert().success();
    cmd(d.path()).args(["status", "--json"]).assert().success();
    cmd(d.path())
        .arg("add")
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!("tagteam: {LOCKED}\n"));
    let out = cmd(d.path())
        .args(["add-token", "sk-ant-api03-key", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "keychain-locked", "message": LOCKED}})
    );
    assert_home_empty(d.path());
    // Linux has no Keychain, so no check.
    cmd(d.path())
        .args(["add-token", "sk-ant-api03-key"])
        .env("TAGTEAM_TEST_PLATFORM", "linux")
        .assert()
        .success();
}

#[test]
fn a_routine_switch_is_quiet_and_debug_shows_the_diagnostics() {
    // Claude Code rotated the live credential, and M1 has no oracle to attribute it, so every
    // such switch captures it with a WARN log line (§9.4 step 4). That is a diagnostic: the
    // default level keeps it off stderr, and `--debug` shows it.
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = FileKeychain::new(d.path().join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b-rotated");
    let out = cmd(d.path())
        .args(["switch", "1", "--json"])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    // Exactly one object: anything after it would fail to parse.
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        (v["switched"].clone(), v["to"].clone()),
        (json!(true), json!(1))
    );
    login(&env, &kc, "a@x.co", "", "rt-a-rotated");
    let out = cmd(d.path())
        .args(["switch", "2", "--json", "--debug"])
        .assert()
        .success()
        .stderr(predicates::str::contains(
            "captured an unverified live credential",
        ))
        .get_output()
        .stdout
        .clone();
    assert_eq!(serde_json::from_slice::<Value>(&out).unwrap()["to"], 2);
}

#[test]
fn a_switch_asks_the_configured_profile_endpoint() {
    // §7.6 through the binary: the oracle answer attributes b's rotation to b, so the switch
    // captures it into b's vault; the request carried the bearer and tagteam's User-Agent.
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = FileKeychain::new(d.path().join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b2"); // CC rotated b in place
    let server = MockServer::start();
    server.on(
        "GET",
        "/api/oauth/profile",
        MockReply::Json {
            status: 200,
            body: json!({"account": {"uuid": "uuid-b@x.co-", "email": "b@x.co"}, "organization": {"uuid": ""}}),
        },
    );
    cmd(d.path())
        .env("TAGTEAM_TEST_API_BASE", server.base_url())
        .args(["switch", "1", "--json"])
        .assert()
        .success();
    assert_eq!(server.hits("GET", "/api/oauth/profile"), 1);
    let req = &server.requests()[0];
    let header = |name: &str| {
        req.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    assert_eq!(header("authorization").as_deref(), Some("Bearer at"));
    assert!(header("user-agent").unwrap().starts_with("tagteam/"));
    let list: Value = serde_json::from_slice(
        &cmd(d.path())
            .args(["list", "--json"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let b_id = list["accounts"][1]["id"].as_str().unwrap().to_owned();
    let stored = kc.find("tagteam", &b_id).present().unwrap();
    assert!(String::from_utf8(stored).unwrap().contains("rt-b2"));
}

#[test]
fn by_default_the_test_binary_never_reaches_the_network() {
    // The harness points every endpoint at OFFLINE_API_BASE: a switch that asks the oracle
    // gets `PreSend` at once and still completes (the oracle is advisory, §7.6).
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = FileKeychain::new(d.path().join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b2");
    let started = std::time::Instant::now();
    cmd(d.path()).args(["switch", "1"]).assert().success();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn a_test_base_never_goes_through_the_environment_proxy() {
    // A test build with an API base sends direct: even with a proxy in the environment (and
    // no NO_PROXY), the oracle request never reaches it, so no test traffic leaves the machine.
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    let kc = FileKeychain::new(d.path().join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(d.path()).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b2"); // the switch asks the oracle about this token
    let proxy = MockServer::start();
    let proxy_url = proxy.base_url();
    let target = MockServer::start();
    target.on(
        "GET",
        "/api/oauth/profile",
        MockReply::Json {
            status: 200,
            body: json!({"account": {"uuid": "uuid-b@x.co-", "email": "b@x.co"}}),
        },
    );
    cmd(d.path())
        .env("TAGTEAM_TEST_API_BASE", target.base_url())
        .env("ALL_PROXY", &proxy_url)
        .env("HTTP_PROXY", &proxy_url)
        .env("HTTPS_PROXY", &proxy_url)
        .args(["switch", "1"])
        .assert()
        .success();
    assert_eq!(
        target.hits("GET", "/api/oauth/profile"),
        1,
        "asked directly"
    );
    assert_eq!(proxy.requests().len(), 0, "the proxy was never consulted");
}

/// The binary with every endpoint pointed at Task 7's `common::OFFLINE_API_BASE` (connection
/// refused: every endpoint fails `PreSend`, and nothing reaches the network). `cmd` already sets
/// it; naming it here keeps each test's reliance on it visible.
fn offline(root: &Path) -> assert_cmd::Command {
    let mut c = cmd(root);
    c.env("TAGTEAM_TEST_API_BASE", common::OFFLINE_API_BASE);
    c
}

fn quarantine(root: &Path, id: &str) {
    Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
        .set_quarantine(&AccountId::from_string(id), "invalid_grant", "sha256:0", 1)
        .unwrap();
}

#[test]
fn list_and_status_mark_quarantined_accounts() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = two_accounts(d.path());
    quarantine(d.path(), &a);
    quarantine(d.path(), &b);
    offline(d.path()).arg("list").assert().success().stdout(
        "    #  ACCOUNT\n    1  a@x.co   relogin required\n *  2  b@x.co   relogin required\n",
    );
    offline(d.path())
        .arg("status")
        .assert()
        .success()
        .stdout("Live: b@x.co (position 2 of 2), relogin required\n");
    let out = offline(d.path()).args(["list", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["accounts"][0]["usageStatus"], "relogin_required");
}

#[test]
fn a_quarantined_target_that_needs_a_refresh_is_refused_with_a_relogin_message() {
    // §7.2: a quarantined target is never refreshed; one that would need it is refused as
    // Dead is, before anything is locked or written.
    const MESSAGE: &str = "a@x.co (position 1) needs a new login: its stored refresh token can no longer be used; log in with `claude`, then run `tagteam add`";
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    quarantine(d.path(), &a);
    expire_vault(d.path(), &a, 60_000);
    offline(d.path())
        .args(["switch", "1"])
        .assert()
        .code(1)
        .stderr(format!("tagteam: {MESSAGE}\n"));
    let out = offline(d.path())
        .args(["switch", "1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "relogin-required", "message": MESSAGE}})
    );
    assert_eq!(live_email(d.path()), "b@x.co");
}

#[test]
fn an_unreadable_rescue_blocks_the_switch_and_names_the_file() {
    // Review Focus 4, through the binary: the vault's generation may be consumed, so it is
    // never activated while a rescue for the account cannot be read (§6.2).
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    let dir = Env::for_test(d.path()).data_dir().join("rescue");
    std::fs::create_dir_all(&dir).unwrap();
    let name = format!("{a}-0-000000000000.json");
    std::fs::write(dir.join(&name), "{ truncated").unwrap();

    let out = offline(d.path())
        .args(["switch", "1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["error"]["type"], "rescue-pending");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with(
            "a@x.co (position 1) has a refreshed token that is not in the vault yet: "
        ),
        "{message}"
    );
    assert!(message.contains(&name), "{message}");
    assert!(
        message.ends_with("; retry once the vault can be written"),
        "{message}"
    );
    offline(d.path())
        .args(["switch", "1"])
        .assert()
        .code(1)
        .stderr(predicates::str::starts_with(
            "tagteam: a@x.co (position 1) has a refreshed token that is not in the vault yet: ",
        ));
    assert_eq!(live_email(d.path()), "b@x.co");
}

#[test]
fn an_offline_refresh_before_a_switch_warns_once_and_still_switches() {
    // Review Focus 1, through the binary: a transient failure proceeds with the vault's
    // generation and a warning (§7.2), on stderr and in `warnings`, and never names a token.
    let d = tempfile::tempdir().unwrap();
    let (a, _) = two_accounts(d.path());
    expire_vault(d.path(), &a, 60_000);

    let out = offline(d.path())
        .args(["switch", "1", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();

    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        (v["switched"].clone(), v["to"].clone()),
        (json!(true), json!(1))
    );
    let warnings = v["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.matches("warning: ").count(), 1, "{stderr}");
    assert!(stderr.contains(warnings[0].as_str().unwrap()), "{stderr}");
    assert!(!stderr.contains("rt-a"), "never a token: {stderr}");
    assert_eq!(live_email(d.path()), "a@x.co");
}
