//! `tagteam history` through the binary (§13.4). Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::path::Path;

use common::{
    cmd, login, now_epoch_s, record_reading, seed_home, two_fresh_accounts, usage_window,
};
use serde_json::{Value, json};
use tagteam_cc::usage::format_iso8601 as iso;
use tagteam_core::pace::pace;
use tagteam_core::{ProjectionMethod, Sample, WindowKind};
use tagteam_provider::mock_server::{MockReply, MockServer};
use tagteam_provider::{Env, FileKeychain};

const HOUR: i64 = 3_600;
const WEEK: i64 = 604_800;

/// `b` (live) read three times, two hours apart, the last two hours ago, with each window on one
/// instance throughout. `a` is never read.
struct Seeded {
    dir: tempfile::TempDir,
    b: String,
    at: [i64; 3],
    r7: i64,
}

impl Seeded {
    fn root(&self) -> &Path {
        self.dir.path()
    }
}

fn seeded() -> Seeded {
    let dir = tempfile::tempdir().unwrap();
    let (_, b) = two_fresh_accounts(dir.path());
    let now = now_epoch_s();
    let at = [now - 6 * HOUR, now - 4 * HOUR, now - 2 * HOUR];
    // Well clear of a unit boundary, so a countdown read seconds later prints the same text.
    let (r5, r7) = (now + 2 * HOUR + 40 * 60 + 30, now + 3 * 86_400 + 30 * 60);
    for (i, t) in at.into_iter().enumerate() {
        let i = i as f64;
        record_reading(
            dir.path(),
            &b,
            t,
            &[
                usage_window(
                    "5h",
                    "5h",
                    WindowKind::Short,
                    5.0 + 2.0 * i,
                    Some(r5),
                    Some(5 * HOUR),
                ),
                usage_window(
                    "7d",
                    "7d",
                    WindowKind::Long,
                    10.0 + 10.0 * i,
                    Some(r7),
                    Some(WEEK),
                ),
                usage_window("spend", "spend", WindowKind::Spend, 0.0, None, None),
                usage_window(
                    "scoped:Fable",
                    "Fable",
                    WindowKind::Scoped,
                    i,
                    Some(r7),
                    Some(WEEK),
                ),
            ],
        );
    }
    Seeded { dir, b, at, r7 }
}

fn history_json(root: &Path, args: &[&str]) -> Value {
    let out = cmd(root)
        .arg("history")
        .args(args)
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).unwrap()
}

fn keys(v: &Value) -> Vec<String> {
    v["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["key"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn history_defaults_to_the_live_account_and_its_relevant_windows() {
    let s = seeded();
    let v = history_json(s.root(), &[]);
    assert_eq!(
        (v["schemaVersion"].clone(), v["provider"].clone()),
        (json!(1), json!("claude-code"))
    );
    assert_eq!(
        v["account"],
        json!({"number": 2, "id": s.b, "email": "b@x.co"})
    );
    // Spend never counts, and no model is configured (§8.2).
    assert_eq!(keys(&v), ["5h", "7d"]);
    // The 7d window as stored, with the pace of its last reading: the engine calls the same
    // pure function over the same stored values.
    let samples: Vec<Sample> =
        s.at.iter()
            .zip([10.0, 20.0, 30.0])
            .map(|(&fetched_at, pct)| Sample {
                fetched_at,
                pct,
                resets_at: Some(s.r7),
            })
            .collect();
    let current = usage_window("7d", "7d", WindowKind::Long, 30.0, Some(s.r7), Some(WEEK));
    let p = pace(&current, s.at[2], &samples);
    assert_eq!(p.method, Some(ProjectionMethod::Regression), "{p:?}");
    assert!((p.rate_per_hour.unwrap() - 5.0).abs() < 1e-9, "{p:?}");
    let stored: Vec<Value> = samples
        .iter()
        .map(|x| json!({"fetchedAt": iso(x.fetched_at), "pct": x.pct, "resetsAt": iso(s.r7)}))
        .collect();
    assert_eq!(
        v["windows"][1],
        json!({
            "key": "7d", "label": "7d", "kind": "long", "pct": 30.0, "resetsAt": iso(s.r7),
            "samples": stored,
            "ratePerHour": p.rate_per_hour, "expectedPct": p.expected_pct, "aheadOfPace": p.ahead,
            "projectedExhaustionAt": p.exhaustion_at.map(iso),
            "willLastToReset": p.will_last_to_reset, "projectionMethod": "regression",
        })
    );
}

#[test]
fn a_configured_model_is_relevant_by_default() {
    let s = seeded();
    let config = Env::for_test(s.root()).config_dir();
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "[autoswitch]\nmodels = [\"Fable\"]\n",
    )
    .unwrap();
    assert_eq!(
        keys(&history_json(s.root(), &[])),
        ["5h", "7d", "scoped:Fable"]
    );
}

#[test]
fn window_picks_one_window_by_key_or_label_and_since_bounds_the_samples() {
    let s = seeded();
    assert_eq!(
        keys(&history_json(s.root(), &["--window", "fable"])),
        ["scoped:Fable"]
    );
    assert_eq!(
        keys(&history_json(s.root(), &["--window", "SPEND"])),
        ["spend"]
    );
    let v = history_json(s.root(), &["--window", "7d", "--since", "3h"]);
    let fetched: Vec<&str> = v["windows"][0]["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["fetchedAt"].as_str().unwrap())
        .collect();
    assert_eq!(fetched, [iso(s.at[2])]);
}

#[test]
fn csv_prints_the_raw_samples() {
    let s = seeded();
    let out = cmd(s.root())
        .args(["history", "--window", "7d", "--csv"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let mut expected = String::from("window,fetched_at,pct,resets_at\n");
    for (t, pct) in s.at.iter().zip(["10", "20", "30"]) {
        expected.push_str(&format!("7d,{},{pct},{}\n", iso(*t), iso(s.r7)));
    }
    assert_eq!(String::from_utf8(out).unwrap(), expected);
}

#[test]
fn the_text_shows_sparklines_rates_and_projections() {
    let s = seeded();
    let out = cmd(s.root())
        .arg("history")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    // 5h climbs a point an hour and lasts to its reset; 7d climbs five and runs out about 12
    // hours after its last reading, a countdown whose exact minute depends on the wall clock.
    assert!(
        text.starts_with(concat!(
            "b@x.co (position 2), last 7d\n\n",
            "5h  9%  resets in 2h40m\n  ▁▁▂  3 samples\n  +1.0 pts/h · lasts to reset (regression)\n\n",
            "7d  30%  resets in 3d00h\n  ▂▂▃  3 samples\n  +5.0 pts/h · runs out in ",
        )),
        "{text}"
    );
    assert!(text.ends_with(" (regression)\n"), "{text}");
    assert_eq!(
        text.matches(" samples\n").count(),
        2,
        "only relevant windows: {text}"
    );
}

#[test]
fn an_account_without_samples_says_so() {
    let s = seeded();
    cmd(s.root())
        .args(["history", "1"])
        .assert()
        .success()
        .stdout("No usage history for a@x.co.\n");
    assert_eq!(history_json(s.root(), &["1"])["windows"], json!([]));
}

#[test]
fn without_a_live_login_history_needs_an_account() {
    const NO_LOGIN: &str = "there is no live login; name an account, or log in with `claude` first";
    let s = seeded();
    let env = Env::for_test(s.root());
    seed_home(&env);
    cmd(s.root())
        .arg("history")
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!("tagteam: {NO_LOGIN}\n"));
    let out = cmd(s.root())
        .args(["history", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "no-live-login", "message": NO_LOGIN}})
    );
    login(
        &env,
        &FileKeychain::new(s.root().join("keychain")),
        "c@x.co",
        "",
        "rt-c",
    );
    cmd(s.root()).arg("history").assert().code(1).stderr(
        "tagteam: the live login (c@x.co) is not managed by tagteam; name an account, or run `tagteam add` first\n",
    );
    // Naming the account needs no live login.
    assert_eq!(history_json(s.root(), &["2"])["account"]["email"], "b@x.co");
}

#[test]
fn history_never_fetches() {
    // Both accounts are due for a fetch: `b`'s reading is two hours old and `a` has none.
    let s = seeded();
    let server = MockServer::start();
    server.on(
        "GET",
        "/api/oauth/usage",
        MockReply::Json {
            status: 200,
            body: json!({}),
        },
    );
    let cases: [&[&str]; 3] = [&["history"], &["history", "--json"], &["history", "--csv"]];
    for args in cases {
        cmd(s.root())
            .env("TAGTEAM_TEST_API_BASE", server.base_url())
            .args(args)
            .assert()
            .success();
    }
    assert_eq!(server.requests().len(), 0);
}

#[test]
fn bad_flags_are_usage_errors() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("home")).unwrap();
    for since in ["7w", "0d", "d", "1.5h", "7"] {
        cmd(d.path())
            .args(["history", "--since", since])
            .assert()
            .code(2)
            .stdout("")
            .stderr("tagteam: --since takes a span like 14d, 12h or 30m\n");
    }
    let out = cmd(d.path())
        .args(["history", "--csv", "--json"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "usage",
               "message": "--csv and --json are two output formats; pass one"}})
    );
}

#[test]
fn help_documents_the_json_shape() {
    // §13.2: `history`'s `--json` shape is documented in `--help`.
    let d = tempfile::tempdir().unwrap();
    let out = cmd(d.path())
        .args(["history", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(out).unwrap();
    for word in [
        "never fetches",
        "fetchedAt",
        "ratePerHour",
        "projectionMethod",
    ] {
        assert!(help.contains(word), "{word}: {help}");
    }
}
