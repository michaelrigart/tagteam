//! `list` and `status` with usage, through the real binary against a `MockServer` that serves
//! the recorded usage reply (§8.3, §13.1, §13.2). Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::path::Path;

use common::{cmd, login, now_epoch_s, seed_home};
use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601;
use tagteam_core::{CLAUDE_CODE, PollBudget, ProviderId};
use tagteam_engine::store::{Eligibility, Reserve, SendGrant, Store};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Env, FileKeychain};

const API_BASE: &str = "TAGTEAM_TEST_API_BASE";
const USAGE: &str = "/api/oauth/usage";

/// The recorded usage reply (`tagteam-cc`'s fixture) with its windows' resets moved to fixed
/// distances from `now`: 5h in 2h40m30s, and 7d and Fable in 3d09h00m30s. The 30 s keep each
/// countdown on its minute while the binary runs, and 77 % three and a half days into the week
/// is ahead of pace (§8.7).
fn usage_body(now: i64) -> Value {
    let recorded: Value = serde_json::from_str(include_str!(
        "../../tagteam-cc/tests/fixtures/endpoints/usage-200.json"
    ))
    .unwrap();
    let mut body = recorded["body"].clone();
    body["five_hour"]["resets_at"] = json!(format_iso8601(now + 9_630));
    body["seven_day"]["resets_at"] = json!(format_iso8601(now + 291_630));
    body["limits"][2]["resets_at"] = json!(format_iso8601(now + 291_630));
    body
}

/// A server whose usage endpoint answers `reply`, every time.
fn serving(reply: MockReply) -> MockServer {
    let server = MockServer::start();
    server.on("GET", USAGE, reply);
    server
}

fn recorded_reply(now: i64) -> MockReply {
    MockReply::Json {
        status: 200,
        body: usage_body(now),
    }
}

/// Logs each email in and stores it with `add`, which sends no usage request (only `list` and
/// `status` collect); the last one stays live.
fn accounts(root: &Path, emails: &[&str]) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    for (i, email) in emails.iter().enumerate() {
        login(&env, &kc, email, "", &format!("rt-{i}"));
        cmd(root).arg("add").assert().success();
    }
}

/// `tagteam <args>` against `server`: it must succeed. Returns stdout and stderr.
fn run(root: &Path, server: &MockServer, args: &[&str]) -> (String, String) {
    let out = cmd(root)
        .env(API_BASE, server.base_url())
        .args(args)
        .assert()
        .success()
        .get_output()
        .clone();
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn json_of(root: &Path, server: &MockServer, args: &[&str]) -> Value {
    serde_json::from_str(&run(root, server, args).0).unwrap()
}

const TABLE: &str = concat!(
    "    #  ACCOUNT  5H           7D                   SPEND  FABLE        AGE\n",
    "    1  a@x.co     9%  2h40m   77%  3d09h  ▲ pace  —        0%  3d09h  <1m\n",
    " *  2  b@x.co     9%  2h40m   77%  3d09h  ▲ pace  —        0%  3d09h  <1m\n",
);

#[test]
fn list_fetches_each_account_and_shows_its_windows() {
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co", "b@x.co"]);
    let server = serving(recorded_reply(now_epoch_s()));
    let (out, err) = run(d.path(), &server, &["list"]);
    assert_eq!((out.as_str(), err.as_str()), (TABLE, ""));
    assert_eq!(server.hits("GET", USAGE), 2);
    // §8.1's request: the account's bearer token, the beta header, tagteam's User-Agent.
    for req in server.requests() {
        let header = |name: &str| {
            req.headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(header("authorization").as_deref(), Some("Bearer at"));
        assert_eq!(
            header("anthropic-beta").as_deref(),
            Some("oauth-2025-04-20")
        );
        assert!(header("user-agent").unwrap().starts_with("tagteam/"));
    }
}

#[test]
fn a_second_list_within_the_floor_sends_no_request() {
    // §8.3's on-demand rule: a reading younger than 180 s is served, not fetched again.
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co", "b@x.co"]);
    let server = serving(recorded_reply(now_epoch_s()));
    run(d.path(), &server, &["list"]);
    assert_eq!(run(d.path(), &server, &["list"]).0, TABLE);
    let v = json_of(d.path(), &server, &["list", "--json"]);
    assert_eq!(server.hits("GET", USAGE), 2, "one request per account");
    for row in v["accounts"].as_array().unwrap() {
        assert_eq!(row["usageStatus"], "ok");
        assert!(row["usage"].is_object(), "decision-grade: {row}");
    }
}

#[test]
fn list_json_rows_carry_usage_as_section_13_2_says() {
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co"]);
    let server = serving(recorded_reply(now_epoch_s()));
    let before = now_epoch_s();
    let v = json_of(d.path(), &server, &["list", "--json"]);
    let after = now_epoch_s();
    let row = &v["accounts"][0];
    let keys: Vec<&str> = row
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "number",
            "position",
            "id",
            "provider",
            "email",
            "organizationName",
            "organizationUuid",
            "isOrganization",
            "active",
            "usageStatus",
            "usage",
            "usageFetchedAt",
            "usageAgeSeconds",
            "loginExpiresAt",
        ]
    );
    assert_eq!(row["usageStatus"], "ok");
    let usage = &row["usage"];
    assert_eq!(usage["fiveHour"]["pct"].as_f64(), Some(9.0));
    assert_eq!(usage["sevenDay"]["pct"].as_f64(), Some(77.0));
    assert_eq!(usage["sevenDay"]["aheadOfPace"], true);
    assert_eq!(usage["scoped"][0]["name"], "Fable");
    let fetched = row["usageFetchedAt"].as_str().unwrap();
    assert!(
        (format_iso8601(before).as_str()..=format_iso8601(after).as_str()).contains(&fetched),
        "{fetched}"
    );
    assert!(row["usageAgeSeconds"].as_i64().unwrap() <= after - before);

    // `status` carries the same row, managed; the reading is fresh, so nothing is fetched.
    let s = json_of(d.path(), &server, &["status", "--json"]);
    assert_eq!(
        (&s["active"]["managed"], &s["active"]["usage"]),
        (&json!(true), usage)
    );
    assert_eq!(server.hits("GET", USAGE), 1);
}

#[test]
fn a_429_is_a_row_that_says_when_it_is_retried_never_a_command_error() {
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co"]);
    let server = serving(MockReply::Raw {
        status: 429,
        headers: vec![("retry-after".into(), "330".into())],
        body: b"{}".to_vec(),
    });
    let before = now_epoch_s();
    let (out, _) = run(d.path(), &server, &["list"]);
    assert_eq!(
        out,
        "    #  ACCOUNT\n *  1  a@x.co   unavailable (http-429, retry 5m)\n"
    );
    // In backoff: the next list sends nothing and reports the same.
    let v = json_of(d.path(), &server, &["list", "--json"]);
    let after = now_epoch_s();
    assert_eq!(server.hits("GET", USAGE), 1);
    let row = &v["accounts"][0];
    assert_eq!(
        (&row["usageStatus"], &row["usage"], &row["lastGoodUsage"]),
        (&json!("unavailable"), &Value::Null, &Value::Null)
    );
    assert_eq!(
        (&row["lastGoodFetchedAt"], &row["lastGoodAgeSeconds"]),
        (&Value::Null, &Value::Null)
    );
    assert_eq!(row["usageError"], "http-429");
    let retry = row["usageRetryAt"].as_str().unwrap();
    let (earliest, latest) = (format_iso8601(before + 330), format_iso8601(after + 330));
    assert!(
        (earliest.as_str()..=latest.as_str()).contains(&retry),
        "{retry}"
    );
}

#[test]
fn an_unread_account_over_its_hourly_budget_reads_as_over_budget_not_no_data() {
    // §8.6, Review Focus 4: an account removed and re-added within the hour has no reading,
    // but its identity's budget is spent. The fetch is refused before anything is sent, and
    // its row says so, with when a slot frees, instead of "no data yet".
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co"]);
    let spent_at = now_epoch_s() - 100;
    let store = Store::open_existing(&Env::for_test(d.path()).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let row = store.accounts(&ProviderId::new(CLAUDE_CODE)).unwrap()[0].clone();
    let Reserve::Reserved(r) = store
        .reserve_usage(
            &row,
            spent_at * 1000,
            Eligibility::Scheduled,
            &PollBudget::STANDARD,
        )
        .unwrap()
    else {
        panic!("the first slot is free");
    };
    while let SendGrant::Send(_) = store
        .authorize_send(&r, None, None, spent_at * 1000, &PollBudget::STANDARD)
        .unwrap()
    {}
    let free = spent_at + PollBudget::STANDARD.count_window_s;
    let server = serving(recorded_reply(now_epoch_s()));

    let (out, _) = run(d.path(), &server, &["list"]);
    assert_eq!(
        out,
        "    #  ACCOUNT\n *  1  a@x.co   over budget (retry 59m)\n"
    );
    let v = json_of(d.path(), &server, &["list", "--json"]);
    let row = &v["accounts"][0];
    assert_eq!(
        (
            &row["usageStatus"],
            &row["usage"],
            &row["usageError"],
            &row["usageRetryAt"]
        ),
        (
            &json!("unavailable"),
            &Value::Null,
            &json!("over-budget"),
            &json!(format_iso8601(free))
        )
    );
    assert_eq!(server.hits("GET", USAGE), 0, "nothing was sent");
}

#[test]
fn status_collects_the_live_account_only_and_shows_its_usage() {
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co", "b@x.co"]);
    let server = serving(recorded_reply(now_epoch_s()));
    let (out, _) = run(d.path(), &server, &["status"]);
    assert_eq!(
        out,
        "Live: b@x.co (position 2 of 2)\n  5h 9% (2h40m) · 7d 77% (3d09h) ▲ pace · Fable 0% (3d09h) · <1m old\n"
    );
    assert_eq!(server.hits("GET", USAGE), 1, "b only");
    run(d.path(), &server, &["list"]);
    assert_eq!(server.hits("GET", USAGE), 2, "then a, b being fresh");
}

#[test]
fn colour_follows_the_setting_and_the_environment() {
    // §13.1: a 77 % window is a warning (yellow). Off on a pipe by default; `ui.color` and
    // `FORCE_COLOR` turn it on; `NO_COLOR` and `--no-color` win over both.
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co"]);
    let server = serving(recorded_reply(now_epoch_s()));
    const YELLOW_77: &str = "\x1b[33m77%\x1b[0m";
    let list = |env: &[(&str, &str)], args: &[&str]| {
        let mut c = cmd(d.path());
        c.env(API_BASE, server.base_url()).arg("list").args(args);
        for (k, v) in env {
            c.env(k, v);
        }
        String::from_utf8(c.assert().success().get_output().stdout.clone()).unwrap()
    };
    assert!(!list(&[], &[]).contains('\x1b'));
    assert!(list(&[("FORCE_COLOR", "1")], &[]).contains(YELLOW_77));
    let config = Env::for_test(d.path()).config_dir();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.toml"), "[ui]\ncolor = \"always\"\n").unwrap();
    assert!(list(&[], &[]).contains(YELLOW_77));
    assert!(!list(&[("NO_COLOR", "1")], &[]).contains('\x1b'));
    assert!(!list(&[("FORCE_COLOR", "1")], &["--no-color"]).contains('\x1b'));
    // An empty variable counts as unset (no-color.org, force-color.org): `NO_COLOR=` does not
    // switch colour off, and `FORCE_COLOR=` does not switch it on.
    assert!(list(&[("NO_COLOR", "")], &[]).contains(YELLOW_77));
    assert!(list(&[("NO_COLOR", ""), ("FORCE_COLOR", "1")], &[]).contains(YELLOW_77));
    std::fs::write(config.join("config.toml"), "[ui]\ncolor = \"never\"\n").unwrap();
    assert!(!list(&[("FORCE_COLOR", "")], &[]).contains('\x1b'));
    assert!(list(&[("FORCE_COLOR", "1")], &[]).contains(YELLOW_77));
    assert_eq!(server.hits("GET", USAGE), 1);
}

#[test]
fn a_collection_that_fails_as_a_whole_is_one_warning_and_never_a_command_error() {
    // §8.3: a usage failure is never a command error. The hook fails the collection before any
    // account starts (a store that cannot be read, say), so only `collect_usage`'s `Err` can
    // answer; the CLI warns once and lists what it has.
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co", "b@x.co"]);
    let server = serving(recorded_reply(now_epoch_s()));
    let failing = |args: &[&str]| {
        let out = cmd(d.path())
            .env(API_BASE, server.base_url())
            .env("TAGTEAM_TEST_FAIL_AT", "usage-collect-start")
            .args(args)
            .assert()
            .success()
            .get_output()
            .clone();
        (
            String::from_utf8(out.stdout).unwrap(),
            String::from_utf8(out.stderr).unwrap(),
        )
    };
    let warning = "warning: usage was not collected: injected failure at usage-collect-start\n";
    let (out, err) = failing(&["list"]);
    assert_eq!(err, warning);
    assert!(out.starts_with("    #  ACCOUNT"), "{out}");
    let (out, err) = failing(&["list", "--json"]);
    assert_eq!(err, warning);
    let v: Value = serde_json::from_str(&out).expect("stdout stays one JSON object");
    assert_eq!(v["accounts"].as_array().unwrap().len(), 2);
    assert_eq!(server.hits("GET", USAGE), 0, "nothing was collected");
}

#[test]
fn a_locked_keychain_row_says_when_it_is_retried_in_words_and_in_json() {
    // §13.1, §13.2: not only `unavailable` rows: any status that is retried says when.
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co", "b@x.co"]);
    std::fs::write(d.path().join("keychain/LOCKED"), "").unwrap();
    let server = serving(recorded_reply(now_epoch_s()));
    let before = now_epoch_s();
    let (out, err) = run(d.path(), &server, &["list"]);
    assert_eq!(err, "");
    assert_eq!(
        out,
        concat!(
            "    #  ACCOUNT\n",
            "    1  a@x.co   keychain unavailable (retry <1m)\n",
            " *  2  b@x.co   keychain unavailable (retry <1m)\n",
        )
    );
    let v = json_of(d.path(), &server, &["list", "--json"]);
    let after = now_epoch_s();
    for row in v["accounts"].as_array().unwrap() {
        assert_eq!(row["usageStatus"], "keychain_unavailable");
        assert_eq!(row["usageError"], "keychain-unavailable");
        let retry = row["usageRetryAt"].as_str().unwrap();
        assert!(
            (format_iso8601(before).as_str()..=format_iso8601(after + 60).as_str())
                .contains(&retry),
            "{retry}"
        );
    }
    assert_eq!(server.hits("GET", USAGE), 0, "nothing could be sent");
}
