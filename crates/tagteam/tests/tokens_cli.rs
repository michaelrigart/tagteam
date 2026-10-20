//! `tagteam tokens` through the binary (token report spec §11, §15 "CLI"): the table, `--json`
//! and `--csv` of every period, the flags, and their usage errors. Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::Path;

use common::{Turn, cmd, now_epoch_s, turn, two_fresh_accounts, write_transcript};
use serde_json::{Value, json};
use tagteam_cc::CLAUDE_CODE;
use tagteam_core::time::format_iso8601;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{NewAccount, SightingRow, SightingSource, Store};
use tagteam_fake::FAKE_AGENT;
use tagteam_provider::{Env, Identity};

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;
const OPUS: &str = "claude-opus-5-5";
const SONNET: &str = "claude-sonnet-5-5";
/// A model no price table names.
const UNPRICED: &str = "claude-test-9";
/// config.toml's prices for the two priced models, unlike any shipped table's, so every cost
/// below is fixed (token report spec §8.2). Per token, in nano-dollars: opus 1000, 2000,
/// 3000, 4000 and 500; sonnet 500, 1000, 1500, 2000 and 250.
const PRICES: &str = concat!(
    "[provider.claude-code.prices.\"claude-opus-5-5\"]\n",
    "input = 1.0\noutput = 2.0\ncache_write_5m = 3.0\ncache_write_1h = 4.0\ncache_read = 0.5\n\n",
    "[provider.claude-code.prices.\"claude-sonnet-5-5\"]\n",
    "input = 0.5\noutput = 1.0\ncache_write_5m = 1.5\ncache_write_1h = 2.0\ncache_read = 0.25\n",
);
const PARTIAL: &str = "the token index has not completed its first refresh, so this report is partial; run `tagteam tokens` without --no-refresh to complete it";
const FOOTNOTES: &str = concat!(
    "\n",
    "No price for claude-test-9: counted without cost (* marks a partial cost).\n",
    "Output is a lower bound for 1 subagent response.\n",
);

/// Midnight UTC, `days` days before today's.
fn midnight(days: i64) -> i64 {
    (now_epoch_s() * 1000).div_euclid(DAY_MS) * DAY_MS - days * DAY_MS
}

/// `ms` as `--json` and `--csv` spell a time.
fn iso(ms: i64) -> String {
    format_iso8601(ms.div_euclid(1000))
}

/// The UTC date of `ms`.
fn date_of(ms: i64) -> String {
    iso(ms)[..10].to_owned()
}

fn store(root: &Path) -> Store {
    Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap()
}

/// `tagteam <args>`'s one JSON object; it must succeed.
fn json_of(root: &Path, args: &[&str]) -> Value {
    let out = cmd(root)
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).unwrap()
}

/// `tagteam <args>`'s stdout; it must succeed with nothing on stderr.
fn text_of(root: &Path, args: &[&str]) -> String {
    let out = cmd(root)
        .args(args)
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out).unwrap()
}

/// `a@x.co` (1) and `b@x.co` (2, live), config.toml's prices, and two sessions' transcripts:
/// - `s-a`, yesterday from 10:00 to 11:00 UTC, which `run` sighted in a's profile: three opus
///   responses, one of them a subagent's snapshot that never got its final count;
/// - `s-d`, two days ago at 10:00 and 10:30 UTC in the default home, a sonnet response and one
///   of a model with no price. It ran before the store existed, so no account can be credited
///   with it (spec §7.2–7.3): it is unattributed.
struct Fixture {
    dir: tempfile::TempDir,
    a: String,
    /// Yesterday's and the day before's midnights, UTC.
    d1: i64,
    d2: i64,
    /// The range every report of the fixture asks for: the day before yesterday's date and
    /// today's.
    since: String,
    until: String,
}

impl Fixture {
    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// `args`, then the fixture's range in UTC, so every expectation is fixed by the fixture
    /// whenever the test runs. The default range is `tokens_cmd`'s unit tests' to pin.
    fn args<'a>(&'a self, args: &[&'a str]) -> Vec<&'a str> {
        let mut all = args.to_vec();
        all.extend([
            "--tz",
            "UTC",
            "--since",
            &self.since,
            "--until",
            &self.until,
        ]);
        all
    }
}

/// `json_of` over the fixture's range.
fn json_in(f: &Fixture, args: &[&str]) -> Value {
    json_of(f.root(), &f.args(args))
}

/// `text_of` over the fixture's range.
fn text_in(f: &Fixture, args: &[&str]) -> String {
    text_of(f.root(), &f.args(args))
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (a, _b) = two_fresh_accounts(root);
    let (d1, d2) = (midnight(1), midnight(2));
    write_transcript(
        root,
        "s-a",
        &[
            turn(OPUS, d1 + 10 * HOUR_MS, [1_200, 3_400, 10_000, 0, 250_000]),
            Turn {
                final_line: false,
                ..turn(OPUS, d1 + 10 * HOUR_MS + 30 * MINUTE_MS, [0, 8, 0, 0, 0])
            },
            turn(OPUS, d1 + 11 * HOUR_MS, [800, 1_600, 0, 20_000, 750_000]),
        ],
    );
    write_transcript(
        root,
        "s-d",
        &[
            turn(SONNET, d2 + 10 * HOUR_MS, [50_000, 72_000, 0, 0, 0]),
            turn(
                UNPRICED,
                d2 + 10 * HOUR_MS + 30 * MINUTE_MS,
                [1_000, 3_000, 0, 0, 0],
            ),
        ],
    );
    let sighted = SightingRow {
        session_id: "s-a".into(),
        home: a.clone(),
        started_at: d1 + 9 * HOUR_MS,
        first_at: d1 + 9 * HOUR_MS,
        last_at: d1 + 12 * HOUR_MS,
        source: SightingSource::Run,
        recorded_at: now_epoch_s() * 1000,
    };
    store(root)
        .record_sighting(&ProviderId::new(CLAUDE_CODE), &sighted, 60_000)
        .unwrap();
    let config = Env::for_test(root).config_dir();
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("config.toml"), PRICES).unwrap();
    Fixture {
        dir,
        a,
        d1,
        d2,
        since: date_of(d2),
        until: date_of(d1 + DAY_MS),
    }
}

/// The last footnote, naming the shipped table's date and the overridden models.
fn price_note(as_of: &str) -> String {
    format!(
        "API cost at list prices as of {as_of}, with config.toml's for claude-opus-5-5, claude-sonnet-5-5.\n"
    )
}

/// The `tokens` object of a row or the total.
fn tokens(input: u64, output: u64, write_5m: u64, write_1h: u64, read: u64) -> Value {
    json!({
        "input": input, "output": output, "cacheWrite5m": write_5m, "cacheWrite1h": write_1h,
        "cacheWrite": write_5m + write_1h, "cacheRead": read,
        "total": input + output + write_5m + write_1h + read, "webSearch": 0,
    })
}

#[test]
fn tokens_day_human_json_and_csv() {
    let f = fixture();
    let v = json_in(&f, &["tokens", "--json"]);
    let as_of = v["prices"]["asOf"].as_str().unwrap().to_owned();
    let (d1, d2) = (date_of(f.d1), date_of(f.d2));
    let a = json!({"kind": "account", "number": 1, "id": f.a, "email": "a@x.co"});
    let unattributed = json!({"kind": "unattributed"});
    assert_eq!(
        v["rows"],
        json!([
            {
                "key": format!("day:{d1}|account:{}", f.a),
                "period": {"kind": "day", "date": d1},
                "account": a, "accounts": [a], "models": [OPUS],
                "tokens": tokens(2_000, 5_008, 10_000, 20_000, 1_000_000),
                "responses": 3, "unfinalized": 1, "costUsd": 0.622016, "costPartial": false,
            },
            {
                "key": format!("day:{d2}|account:unattributed"),
                "period": {"kind": "day", "date": d2},
                "account": unattributed, "accounts": [unattributed],
                "models": [SONNET, UNPRICED],
                "tokens": tokens(51_000, 75_000, 0, 0, 0),
                "responses": 2, "unfinalized": 0, "costUsd": 0.097, "costPartial": true,
            },
        ])
    );
    assert_eq!(
        v["total"],
        json!({
            "tokens": tokens(53_000, 80_008, 10_000, 20_000, 1_000_000),
            "responses": 5, "unfinalized": 1, "costUsd": 0.719016, "costPartial": true,
            "byBasis": {"profile": 1_037_008, "timeline": 0, "unattributed": 126_000},
        })
    );
    assert_eq!(
        [
            &v["schemaVersion"],
            &v["provider"],
            &v["period"],
            &v["grouping"],
            &v["timeZone"],
            &v["since"],
            &v["until"],
            &v["unpricedModels"],
            &v["prices"]["overridden"],
        ],
        [
            &json!(1),
            &json!("claude-code"),
            &json!("day"),
            &json!("account"),
            &json!("UTC"),
            &json!(iso(f.d2)),
            &json!(iso(f.d1 + DAY_MS)),
            &json!([UNPRICED]),
            &json!([OPUS, SONNET]),
        ]
    );
    let index = &v["index"];
    assert!(
        index["refreshedAt"].is_string() && index["changedAt"].is_string(),
        "{index}"
    );
    assert_eq!(
        [
            &index["files"],
            &index["responses"],
            &index["skippedLines"],
            &index["unreadableFiles"],
            &index["oldest"],
            &index["newest"],
        ],
        [
            &json!(2),
            &json!(5),
            &json!(0),
            &json!(0),
            &json!(iso(f.d2 + 10 * HOUR_MS)),
            &json!(iso(f.d1 + 11 * HOUR_MS)),
        ]
    );

    assert_eq!(
        text_in(&f, &["tokens", "--no-refresh"]),
        format!(
            concat!(
                "DAY         ACCOUNT       INPUT  OUTPUT  CACHE WRITE  CACHE READ   TOTAL  API COST\n",
                "{d1}  1 a@x.co       2.0K    5.0K        30.0K        1.0M    1.0M    $0.62\n",
                "{d2}  unattributed  51.0K   75.0K            0           0  126.0K    $0.10*\n",
                "TOTAL                     53.0K   80.0K        30.0K        1.0M    1.2M    $0.72*\n",
                "{notes}{prices}",
            ),
            d1 = d1,
            d2 = d2,
            notes = FOOTNOTES,
            prices = price_note(&as_of),
        )
    );
    assert_eq!(
        text_in(&f, &["tokens", "--no-refresh", "--csv"]),
        format!(
            concat!(
                "period,account,model,input,output,cache_write_5m,cache_write_1h,cache_read,web_search,total,responses,unfinalized,cost_usd\n",
                "{d1},a@x.co,,2000,5008,10000,20000,1000000,0,1037008,3,1,0.622016\n",
                "{d2},unattributed,,51000,75000,0,0,0,0,126000,2,0,0.097\n",
            ),
            d1 = d1,
            d2 = d2,
        )
    );

    // By account and model: a row per model, by name, and a model with no price has no cost.
    let v = json_in(
        &f,
        &["tokens", "--by", "account,model", "--json", "--no-refresh"],
    );
    let rows: Vec<(&Value, &Value, &Value, &Value)> = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (&r["key"], &r["model"], &r["costUsd"], &r["costPartial"]))
        .collect();
    assert_eq!(
        rows,
        [
            (
                &json!(format!("day:{d1}|account:{}|model:{OPUS}", f.a)),
                &json!(OPUS),
                &json!(0.622016),
                &json!(false),
            ),
            (
                &json!(format!("day:{d2}|account:unattributed|model:{SONNET}")),
                &json!(SONNET),
                &json!(0.097),
                &json!(false),
            ),
            (
                &json!(format!("day:{d2}|account:unattributed|model:{UNPRICED}")),
                &json!(UNPRICED),
                &Value::Null,
                &json!(false),
            ),
        ]
    );
    assert_eq!(
        text_in(&f, &["tokens", "--by", "account,model", "--no-refresh"]),
        format!(
            concat!(
                "DAY         ACCOUNT       MODEL              INPUT  OUTPUT  CACHE WRITE  CACHE READ   TOTAL  API COST\n",
                "{d1}  1 a@x.co      claude-opus-5-5     2.0K    5.0K        30.0K        1.0M    1.0M    $0.62\n",
                "{d2}  unattributed  claude-sonnet-5-5  50.0K   72.0K            0           0  122.0K    $0.10\n",
                "{d2}  unattributed  claude-test-9       1.0K    3.0K            0           0    4.0K        —\n",
                "TOTAL                                        53.0K   80.0K        30.0K        1.0M    1.2M    $0.72*\n",
                "{notes}{prices}",
            ),
            d1 = d1,
            d2 = d2,
            notes = FOOTNOTES,
            prices = price_note(&as_of),
        )
    );
    // By nothing: a row per day, with no `account` key, naming every account it holds.
    let v = json_in(&f, &["tokens", "--by", "none", "--json", "--no-refresh"]);
    assert_eq!(v["grouping"], "none");
    assert_eq!(v["rows"][0].get("account"), None);
    assert_eq!(v["rows"][0]["accounts"], json!([a]));
}

#[test]
fn bare_tokens_is_day() {
    let f = fixture();
    let root = f.root();
    cmd(root).args(["tokens", "--json"]).assert().success();
    let bare = json_in(&f, &["tokens", "--json", "--no-refresh"]);
    let day = json_in(&f, &["tokens", "day", "--json", "--no-refresh"]);
    assert_eq!(bare["period"], "day");
    assert_eq!(
        (&bare["rows"], &bare["total"], &bare["period"]),
        (&day["rows"], &day["total"], &day["period"])
    );
    // An ACCOUNT on a bare `tokens` is day's too.
    let only_a = json_in(&f, &["tokens", "1", "--json", "--no-refresh"]);
    assert_eq!(only_a["period"], "day");
    assert_eq!(only_a["rows"].as_array().unwrap().len(), 1);
    assert_eq!(only_a["rows"][0]["account"]["number"], 1);
    // `--unattributed` alone is the unattributed bucket alone; with an ACCOUNT, both.
    let keys = |args: &[&str]| -> Vec<String> {
        let mut all = vec!["tokens", "--json", "--no-refresh"];
        all.extend_from_slice(args);
        json_in(&f, &all)["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["key"].as_str().unwrap().to_owned())
            .collect()
    };
    let (d1, d2) = (date_of(f.d1), date_of(f.d2));
    assert_eq!(
        keys(&["--unattributed"]),
        [format!("day:{d2}|account:unattributed")]
    );
    assert_eq!(
        keys(&["a@x.co", "--unattributed"]),
        [
            format!("day:{d1}|account:{}", f.a),
            format!("day:{d2}|account:unattributed"),
        ]
    );
    assert!(
        keys(&["2"]).is_empty(),
        "b, the live login, made none of these requests"
    );
}

#[test]
fn a_flag_before_the_period_leaves_it_the_period() {
    let f = fixture();
    let before = json_in(&f, &["tokens", "--json", "month"]);
    let after = json_in(&f, &["tokens", "month", "--json", "--no-refresh"]);
    assert_eq!(before["period"], "month");
    assert_eq!(before["rows"], after["rows"]);
    assert!(
        !before["rows"].as_array().unwrap().is_empty(),
        "the fixture's two days fall in months: {before}"
    );
    assert_eq!(
        json_in(&f, &["tokens", "--no-refresh", "session", "--json"])["period"],
        "session"
    );
    // An account named like a period is an account anywhere but first.
    cmd(f.root())
        .args(["tokens", "1", "month", "--no-refresh"])
        .assert()
        .code(1)
        .stdout("")
        .stderr("tagteam: no account matches \"month\"\n");
}

#[test]
fn a_since_date_is_local_midnight_in_tz() {
    // Review Focus 4: `--since DATE` is that date's midnight in `--tz`, and a day is that
    // zone's calendar day. Asia/Kolkata is UTC+05:30: 18:30 UTC is its next midnight.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    two_fresh_accounts(root);
    let today = midnight(0);
    write_transcript(
        root,
        "s-tz",
        &[
            // Yesterday 18:00 UTC is 23:30 in Kolkata; 19:00 UTC is today's 00:30 there.
            turn(OPUS, today - 6 * HOUR_MS, [100, 0, 0, 0, 0]),
            turn(OPUS, today - 5 * HOUR_MS, [200, 0, 0, 0, 0]),
        ],
    );
    let local_today = date_of(today);
    let v = json_of(
        root,
        &[
            "tokens",
            "--tz",
            "Asia/Kolkata",
            "--since",
            &local_today,
            "--by",
            "none",
            "--json",
        ],
    );
    assert_eq!(v["timeZone"], "Asia/Kolkata");
    assert_eq!(v["since"], iso(today - 5 * HOUR_MS - 30 * MINUTE_MS));
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{v}");
    assert_eq!(
        rows[0]["period"],
        json!({"kind": "day", "date": local_today})
    );
    assert_eq!(rows[0]["tokens"]["input"], 200);
    // The day before, in UTC: both responses fall on it.
    let yesterday = date_of(today - DAY_MS);
    let v = json_of(
        root,
        &[
            "tokens",
            "--tz",
            "UTC",
            "--since",
            &yesterday,
            "--by",
            "none",
            "--json",
            "--no-refresh",
        ],
    );
    assert_eq!(v["since"], iso(today - DAY_MS));
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{v}");
    assert_eq!(rows[0]["period"]["date"], yesterday);
    assert_eq!(rows[0]["tokens"]["input"], 300);
    // `--until` takes the same forms, exclusive.
    let v = json_of(
        root,
        &[
            "tokens",
            "--tz",
            "UTC",
            "--since",
            &iso(today - 6 * HOUR_MS),
            "--until",
            &iso(today - 5 * HOUR_MS),
            "--by",
            "none",
            "--json",
            "--no-refresh",
        ],
    );
    assert_eq!(v["until"], iso(today - 5 * HOUR_MS));
    assert_eq!(v["total"]["tokens"]["input"], 100);
}

#[test]
fn an_unknown_model_is_a_usage_error_listing_models() {
    let f = fixture();
    let root = f.root();
    let listed = "--model nope matches no indexed model; the index has claude-opus-5-5, claude-sonnet-5-5, claude-test-9";
    cmd(root)
        .args(["tokens", "--model", "nope"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(format!("tagteam: {listed}\n"));
    let out = cmd(root)
        .args([
            "tokens",
            "--model",
            "opus",
            "--model",
            "nope",
            "--json",
            "--no-refresh",
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "usage", "message": listed}})
    );
    // A pattern matches every model containing it, whatever the case.
    let v = json_in(&f, &["tokens", "--model", "OPUS", "--json", "--no-refresh"]);
    assert_eq!(v["rows"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["rows"][0]["models"], json!([OPUS]));
    let v = json_in(
        &f,
        &["tokens", "--model", "claude-s", "--json", "--no-refresh"],
    );
    assert_eq!(v["rows"][0]["models"], json!([SONNET]));
    assert_eq!(v["total"]["responses"], 1);
    // An index with nothing in it names no model.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    cmd(d.path())
        .args(["tokens", "--model", "opus"])
        .assert()
        .code(2)
        .stderr("tagteam: --model opus matches no indexed model; the index has none\n");
}

#[test]
fn csv_and_json_together_is_a_usage_error() {
    // Ruling R6, as for `history`; refused before the store is created.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    let out = cmd(d.path())
        .args(["tokens", "--csv", "--json"])
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
    assert!(!Env::for_test(d.path()).data_dir().exists());
}

#[test]
fn no_refresh_reports_the_partial_index_note() {
    // Spec §14: before the first complete refresh the index is partial, and `--no-refresh`
    // says so on stderr.
    let f = fixture();
    let root = f.root();
    let out = cmd(root)
        .args(["tokens", "--no-refresh", "--json"])
        .assert()
        .success()
        .stderr(format!("note: {PARTIAL}\n"))
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["index"]["refreshedAt"], Value::Null);
    assert_eq!(v["rows"], json!([]));
    cmd(root)
        .args(["tokens", "--no-refresh"])
        .assert()
        .success()
        .stdout("No token usage in this range.\n")
        .stderr(format!("note: {PARTIAL}\n"));
    // A refresh completes the index; from then on `--no-refresh` reports it, quietly.
    cmd(root).args(["tokens"]).assert().success().stderr("");
    let text = text_in(&f, &["tokens", "--no-refresh"]);
    assert!(text.starts_with("DAY "), "{text}");
}

#[test]
fn month_defaults_to_twelve_calendar_months() {
    // Spec §11.1: with no --since, `month` starts on the first day of the eleventh month back, at
    // midnight. Which month that is depends on the date, so this asserts the shape alone, and
    // holds if the month turns while it runs; `default_since_ms`'s unit test pins the month.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    two_fresh_accounts(root);
    // A response every 30 days for 13 months: more months than the report spans.
    let now = now_epoch_s() * 1000;
    let turns: Vec<Turn> = (0..14i64)
        .map(|k| turn(OPUS, now - HOUR_MS - k * 30 * DAY_MS, [10, 0, 0, 0, 0]))
        .collect();
    write_transcript(root, "s-m", &turns);
    let month = |iso: &str| -> i64 {
        iso[..4].parse::<i64>().unwrap() * 12 + iso[5..7].parse::<i64>().unwrap() - 1
    };
    let before = month(&iso(now_epoch_s() * 1000));
    let v = json_of(
        root,
        &["tokens", "month", "--tz", "UTC", "--by", "none", "--json"],
    );
    let after = month(&iso(now_epoch_s() * 1000));
    let since = v["since"].as_str().unwrap();
    assert!(
        since.ends_with("-01T00:00:00Z"),
        "midnight on a first: {since}"
    );
    let first = month(since);
    assert!(
        first == before - 11 || first == after - 11,
        "the eleventh month back: {since}"
    );
    assert_eq!((&v["period"], &v["until"]), (&json!("month"), &Value::Null));
    let months: Vec<i64> = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| month(r["period"]["month"].as_str().unwrap()))
        .collect();
    assert!((2..=12).contains(&months.len()), "{months:?}");
    assert!(
        months.windows(2).all(|w| w[0] > w[1]),
        "a row a month, newest first: {months:?}"
    );
    assert!(
        months.iter().all(|m| (first..=after).contains(m)),
        "{months:?} outside {first}..={after}"
    );
}

#[test]
fn named_accounts_choose_the_provider() {
    // Core §13.1: --provider, else the provider of the accounts named, else the default. This
    // build registers Claude Code alone, so another provider's account is planted in the store.
    let f = fixture();
    let root = f.root();
    let identity = Identity {
        label: "f@x.co".into(),
        email: Some("f@x.co".into()),
        org_uuid: String::new(),
        org_name: None,
        account_uuid: None,
        raw: json!({}),
    };
    store(root)
        .insert_account(&NewAccount {
            id: &AccountId::from_string("0192-fake"),
            provider: &ProviderId::new(FAKE_AGENT),
            position: 1,
            identity_key: "f@x.co\n",
            identity: &identity,
            kind: "oauth",
            alias: Some("fakey"),
            login_expires_at: None,
            added_at: 1,
        })
        .unwrap();
    // Core §10.4: under --provider an alias resolves within that provider only, so another
    // provider's account is as unknown to `tokens` as it is to `run`.
    let refused = cmd(root)
        .args(["run", "--provider", CLAUDE_CODE, "fakey"])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    cmd(root)
        .args(["tokens", "fakey", "--provider", CLAUDE_CODE])
        .assert()
        .failure()
        .stdout("")
        .stderr(String::from_utf8(refused).unwrap());
    // Beside a Claude Code account: one report covers one provider.
    cmd(root)
        .args(["tokens", "a@x.co", "fakey"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(format!(
            "tagteam: the accounts named belong to more than one provider ({CLAUDE_CODE}, {FAKE_AGENT}); name one provider's accounts\n"
        ));
    // Named alone, it chooses its provider: here one this build lacks.
    cmd(root)
        .args(["tokens", "fakey"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!("tagteam: unknown provider \"{FAKE_AGENT}\"\n"));
    // a@x.co chooses Claude Code, and so does naming no account (the default provider).
    let v = json_in(&f, &["tokens", "a@x.co", "--json"]);
    assert_eq!(v["provider"], CLAUDE_CODE);
    assert_eq!(v["rows"].as_array().unwrap().len(), 1, "a's day alone: {v}");
    assert_eq!(
        json_in(&f, &["tokens", "--json", "--no-refresh"])["provider"],
        CLAUDE_CODE
    );
}

#[test]
fn session_and_block_columns() {
    let f = fixture();
    let v = json_in(&f, &["tokens", "session", "--json"]);
    let as_of = v["prices"]["asOf"].as_str().unwrap().to_owned();
    let (d1, d2) = (date_of(f.d1), date_of(f.d2));
    assert_eq!(v["period"], "session");
    assert_eq!(
        (&v["rows"][0]["key"], &v["rows"][1]["key"]),
        (
            &json!(format!("session:s-a|account:{}", f.a)),
            &json!("session:s-d|account:unattributed")
        )
    );
    assert_eq!(
        (&v["rows"][0]["period"], &v["rows"][1]["period"]),
        (
            &json!({"kind": "session", "sessionId": "s-a", "project": "/w",
                    "first": iso(f.d1 + 10 * HOUR_MS), "last": iso(f.d1 + 11 * HOUR_MS)}),
            &json!({"kind": "session", "sessionId": "s-d", "project": "/w",
                    "first": iso(f.d2 + 10 * HOUR_MS), "last": iso(f.d2 + 10 * HOUR_MS + 30 * MINUTE_MS)}),
        )
    );
    assert_eq!(
        text_in(&f, &["tokens", "session", "--no-refresh"]),
        format!(
            concat!(
                "SESSION  FIRST             LAST              ACCOUNT       INPUT  OUTPUT  CACHE WRITE  CACHE READ   TOTAL  API COST  PROJECT\n",
                "s-a      {d1} 10:00  {d1} 11:00  1 a@x.co       2.0K    5.0K        30.0K        1.0M    1.0M    $0.62   /w\n",
                "s-d      {d2} 10:00  {d2} 10:30  unattributed  51.0K   75.0K            0           0  126.0K    $0.10*  /w\n",
                "TOTAL                                                      53.0K   80.0K        30.0K        1.0M    1.2M    $0.72*\n",
                "{notes}{prices}",
            ),
            d1 = d1,
            d2 = d2,
            notes = FOOTNOTES,
            prices = price_note(&as_of),
        )
    );
    assert_eq!(
        text_in(&f, &["tokens", "session", "--no-refresh", "--csv"]),
        format!(
            concat!(
                "period,account,model,input,output,cache_write_5m,cache_write_1h,cache_read,web_search,total,responses,unfinalized,cost_usd,project,first_at,last_at\n",
                "s-a,a@x.co,,2000,5008,10000,20000,1000000,0,1037008,3,1,0.622016,/w,{d1}T10:00:00Z,{d1}T11:00:00Z\n",
                "s-d,unattributed,,51000,75000,0,0,0,0,126000,2,0,0.097,/w,{d2}T10:00:00Z,{d2}T10:30:00Z\n",
            ),
            d1 = d1,
            d2 = d2,
        )
    );

    // Blocks are per account (spec §9): no usage sample observed a reset, so each is inferred,
    // from the hour of its first response, five hours long.
    let v = json_in(&f, &["tokens", "block", "--json", "--no-refresh"]);
    assert_eq!(v["period"], "block");
    assert_eq!(
        (&v["rows"][0]["key"], &v["rows"][1]["key"]),
        (
            &json!(format!("block:{}|account:{}", f.d1 + 10 * HOUR_MS, f.a)),
            &json!(format!(
                "block:{}|account:unattributed",
                f.d2 + 10 * HOUR_MS
            )),
        )
    );
    assert_eq!(
        v["rows"][0]["period"],
        json!({"kind": "block", "start": iso(f.d1 + 10 * HOUR_MS),
               "end": iso(f.d1 + 15 * HOUR_MS), "observed": false, "active": null})
    );
    assert_eq!(
        text_in(&f, &["tokens", "block", "--no-refresh"]),
        format!(
            concat!(
                "START             END               ACCOUNT       INPUT  OUTPUT  CACHE WRITE  CACHE READ   TOTAL  API COST\n",
                "{d1} 10:00  {d1} 15:00  1 a@x.co       2.0K    5.0K        30.0K        1.0M    1.0M    $0.62\n",
                "{d2} 10:00  {d2} 15:00  unattributed  51.0K   75.0K            0           0  126.0K    $0.10*\n",
                "TOTAL                                             53.0K   80.0K        30.0K        1.0M    1.2M    $0.72*\n",
                "{notes}{prices}",
            ),
            d1 = d1,
            d2 = d2,
            notes = FOOTNOTES,
            prices = price_note(&as_of),
        )
    );
    assert_eq!(
        text_in(&f, &["tokens", "block", "--no-refresh", "--csv"]),
        format!(
            concat!(
                "period,account,model,input,output,cache_write_5m,cache_write_1h,cache_read,web_search,total,responses,unfinalized,cost_usd,start,end,observed,active\n",
                "{d1}T10:00:00Z,a@x.co,,2000,5008,10000,20000,1000000,0,1037008,3,1,0.622016,{d1}T10:00:00Z,{d1}T15:00:00Z,false,false\n",
                "{d2}T10:00:00Z,unattributed,,51000,75000,0,0,0,0,126000,2,0,0.097,{d2}T10:00:00Z,{d2}T15:00:00Z,false,false\n",
            ),
            d1 = d1,
            d2 = d2,
        )
    );
}

#[test]
fn json_grouping_is_the_one_applied_not_the_one_requested() {
    // Spec §10.1: blocks are per account, so a block report forces `none` to `account` and
    // `model` to `account,model`; `--json` says what its rows carry. Other periods keep the
    // requested grouping.
    let f = fixture();
    // The first report indexes the fixture's transcripts; the rest read the index.
    json_in(&f, &["tokens", "day", "--json"]);
    for (period, by, want) in [
        ("block", "none", "account"),
        ("block", "model", "account,model"),
        ("block", "account", "account"),
        ("block", "account,model", "account,model"),
        ("day", "none", "none"),
        ("session", "model", "model"),
    ] {
        let v = json_in(
            &f,
            &["tokens", period, "--by", by, "--json", "--no-refresh"],
        );
        assert_eq!(v["grouping"], want, "{period} --by {by}: {v}");
    }
    let v = json_in(
        &f,
        &["tokens", "block", "--by", "none", "--json", "--no-refresh"],
    );
    assert!(
        v["rows"][0]["account"].is_object(),
        "the rows carry the accounts the grouping names: {v}"
    );
}

#[test]
fn help_documents_the_json_shape() {
    // Core §13.2: a command's `--json` shape is documented in its `--help`.
    let d = tempfile::tempdir().unwrap();
    let out = cmd(d.path())
        .args(["tokens", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(out).unwrap();
    for word in [
        "day",
        "month",
        "session",
        "block",
        "unattributed",
        "--no-refresh",
        "cacheWrite5m",
        "costUsd",
        "unpricedModels",
        "timeZone",
    ] {
        assert!(help.contains(word), "{word}: {help}");
    }
}

#[test]
fn bad_flags_are_usage_errors() {
    // Spec §14: an invalid --since, --until, --tz or --by exits 2, before the store exists.
    const SINCE: &str =
        "--since takes a span like 30d, 12h or 90m, a date YYYY-MM-DD, or an ISO 8601 time";
    const UNTIL: &str =
        "--until takes a span like 30d, 12h or 90m, a date YYYY-MM-DD, or an ISO 8601 time";
    const BY: &str = "--by takes account, model, account,model or none";
    const TZ: &str = "unknown time zone \"Mars/Olympus_Mons\"";
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    let cases: [(&[&str], &str); 9] = [
        (&["tokens", "--since", "7w"], SINCE),
        (&["tokens", "--since", "0d"], SINCE),
        (&["tokens", "--since", "2026-02-30"], SINCE),
        (&["tokens", "--since", "2026-10-14 09:00"], SINCE),
        (&["tokens", "--until", "soon"], UNTIL),
        (&["tokens", "--by", "week"], BY),
        (&["tokens", "--by", "model,account"], BY),
        (&["tokens", "--tz", "Mars/Olympus_Mons"], TZ),
        (&["tokens", "month", "--by", "day"], BY),
    ];
    for (args, message) in cases {
        cmd(d.path())
            .args(args)
            .assert()
            .code(2)
            .stdout("")
            .stderr(format!("tagteam: {message}\n"));
    }
    assert!(
        !Env::for_test(d.path()).data_dir().exists(),
        "every flag is checked before the store is created"
    );
}
