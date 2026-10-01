//! `switch --strategy` and `--model` through the real binary (§9.3, §13.2). Readings are
//! recorded through the store, fresh and with a plan in force, so a strategy's on-demand
//! collection sends nothing; every endpoint stays offline (`std_cmd`'s default). Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::path::Path;

use common::{cmd, login, now_epoch_s, record_reading, seed_home, usage_window};
use serde_json::{Value, json};
use tagteam_core::{CLAUDE_CODE, ProviderId, Window, WindowKind};
use tagteam_engine::store::Store;
use tagteam_provider::{Env, FileKeychain};

/// Logs each email in and stores it with `add`, which sends no usage request; the last one
/// stays live. Returns the ids in position order, read from the store: `list` would collect.
fn accounts(root: &Path, emails: &[&str]) -> Vec<String> {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    for (i, email) in emails.iter().enumerate() {
        login(&env, &kc, email, "", &format!("rt-{i}"));
        cmd(root).arg("add").assert().success();
    }
    let store = Store::open_existing(&env.data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    store
        .accounts(&ProviderId::new(CLAUDE_CODE))
        .unwrap()
        .into_iter()
        .map(|r| r.id.as_str().to_owned())
        .collect()
}

/// A reading taken at `now`: 5h at `five` and 7d at `seven`, the 7d window resetting in
/// 3d09h00m30s. The 30 s keep the countdown on its minute while the binary runs.
fn reading(now: i64, five: f64, seven: f64) -> Vec<Window> {
    vec![
        usage_window(
            "5h",
            "5h",
            WindowKind::Short,
            five,
            Some(now + 9_630),
            Some(18_000),
        ),
        usage_window(
            "7d",
            "7d",
            WindowKind::Long,
            seven,
            Some(now + 291_630),
            Some(604_800),
        ),
    ]
}

/// `tagteam switch <args> --json`, which must succeed: its one JSON object.
fn switch_json(root: &Path, args: &[&str]) -> Value {
    let out = cmd(root)
        .arg("switch")
        .args(args)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).unwrap()
}

#[test]
fn best_switches_to_the_candidate_with_the_most_headroom() {
    let d = tempfile::tempdir().unwrap();
    let ids = accounts(d.path(), &["a@x.co", "b@x.co"]);
    let now = now_epoch_s();
    record_reading(d.path(), &ids[0], now, &reading(now, 10.0, 20.0));
    record_reading(d.path(), &ids[1], now, &reading(now, 10.0, 60.0));
    assert_eq!(
        switch_json(d.path(), &["--strategy", "best"]),
        json!({"schemaVersion": 1, "provider": "claude-code", "switched": true, "from": 2,
               "to": 1, "strategy": "best", "reason": "switched", "message": "Switched to a@x.co",
               "credentialStore": "keychain", "warnings": []})
    );
}

#[test]
fn already_best_is_a_no_op_naming_both_binding_windows() {
    let d = tempfile::tempdir().unwrap();
    let ids = accounts(d.path(), &["a@x.co", "b@x.co"]);
    let now = now_epoch_s();
    record_reading(d.path(), &ids[0], now, &reading(now, 10.0, 70.0));
    record_reading(d.path(), &ids[1], now, &reading(now, 10.0, 40.0));
    assert_eq!(
        switch_json(d.path(), &["--strategy", "best"]),
        json!({"schemaVersion": 1, "provider": "claude-code", "switched": false, "from": 2,
               "to": null, "strategy": "best", "reason": "already-best",
               "message": "b@x.co already has the most headroom (7d at 40%); the best candidate is a@x.co (7d at 70%)",
               "credentialStore": null, "warnings": []})
    );
}

#[test]
fn usage_unavailable_when_no_candidate_has_a_reading() {
    // Neither account was read; the strategy's collection fails offline (`pre-send`), and a
    // usage failure is never a command error (§8.3).
    let d = tempfile::tempdir().unwrap();
    accounts(d.path(), &["a@x.co", "b@x.co"]);
    assert_eq!(
        switch_json(d.path(), &["--strategy", "best"]),
        json!({"schemaVersion": 1, "provider": "claude-code", "switched": false, "from": 2,
               "to": null, "strategy": "best", "reason": "usage-unavailable",
               "message": "no candidate has a usage reading recent enough to rank by",
               "credentialStore": null, "warnings": []})
    );
}

#[test]
fn candidates_exhausted_names_each_candidate_and_the_earliest_reset() {
    let d = tempfile::tempdir().unwrap();
    let ids = accounts(d.path(), &["a@x.co", "b@x.co"]);
    let now = now_epoch_s();
    record_reading(d.path(), &ids[0], now, &reading(now, 10.0, 100.0));
    record_reading(d.path(), &ids[1], now, &reading(now, 10.0, 40.0));
    assert_eq!(
        switch_json(d.path(), &["--strategy", "next-available"]),
        json!({"schemaVersion": 1, "provider": "claude-code", "switched": false, "from": 2,
               "to": null, "strategy": "next-available", "reason": "candidates-exhausted",
               "message": "every candidate is at its limit: a@x.co (7d at 100%); the earliest reset is in 3d09h",
               "credentialStore": null, "warnings": []})
    );
}

#[test]
fn next_available_names_the_binding_window_of_each_account_it_skips() {
    let d = tempfile::tempdir().unwrap();
    let ids = accounts(d.path(), &["a@x.co", "b@x.co", "c@x.co"]);
    let now = now_epoch_s();
    record_reading(d.path(), &ids[0], now, &reading(now, 10.0, 100.0));
    record_reading(d.path(), &ids[1], now, &reading(now, 10.0, 30.0));
    record_reading(d.path(), &ids[2], now, &reading(now, 10.0, 50.0));
    let out = cmd(d.path())
        .args(["switch", "--strategy", "next-available"])
        .assert()
        .success()
        .get_output()
        .clone();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "Switched to b@x.co (position 2).\nClaude Code picks this up within about 30 s; restart it to apply now.\n"
    );
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "warning: skipped a@x.co (position 1): at its limit (7d at 100%)\n"
    );
}

#[test]
fn the_model_flag_counts_a_named_scoped_window() {
    let d = tempfile::tempdir().unwrap();
    let ids = accounts(d.path(), &["a@x.co", "b@x.co"]);
    let now = now_epoch_s();
    let mut a = reading(now, 10.0, 20.0);
    a.push(usage_window(
        "scoped:Fable",
        "Fable",
        WindowKind::Scoped,
        95.0,
        Some(now + 291_630),
        Some(604_800),
    ));
    record_reading(d.path(), &ids[0], now, &a);
    record_reading(d.path(), &ids[1], now, &reading(now, 10.0, 60.0));
    // Trimmed, with the empty name dropped: `Fable` counts, and a has 5 points left.
    let named = switch_json(d.path(), &["--strategy", "best", "--model", " Fable , "]);
    assert_eq!(
        (&named["reason"], &named["message"]),
        (
            &json!("already-best"),
            &json!(
                "b@x.co already has the most headroom (7d at 60%); the best candidate is a@x.co (Fable at 95%)"
            )
        )
    );
    // Without --model, no model window counts (`autoswitch.models` is empty): a has 80.
    let unnamed = switch_json(d.path(), &["--strategy", "best"]);
    assert_eq!(
        (&unnamed["reason"], &unnamed["to"]),
        (&json!("switched"), &json!(1))
    );
}

#[test]
fn model_needs_a_strategy_and_a_strategy_takes_no_account() {
    // Decision 7: both are clap usage errors, exit 2; under --json, one JSON usage error.
    let d = tempfile::tempdir().unwrap();
    let cases: [(&[&str], &str); 2] = [
        (
            &["switch", "--model", "fable"],
            "one or more required arguments were not provided",
        ),
        (
            &["switch", "1", "--strategy", "best"],
            "an argument cannot be used with one or more of the other specified arguments",
        ),
    ];
    for (args, kind) in cases {
        let err = cmd(d.path())
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .stderr
            .clone();
        let err = String::from_utf8(err).unwrap();
        assert!(
            err.starts_with(&format!("error: {kind}")),
            "{args:?}: {err}"
        );
        let out = cmd(d.path())
            .args(args)
            .arg("--json")
            .assert()
            .code(2)
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json!({"schemaVersion": 1, "error": {"type": "usage", "message": kind}}),
            "{args:?}"
        );
    }
    cmd(d.path())
        .args(["switch", "--strategy", "fastest"])
        .assert()
        .code(2);
}
