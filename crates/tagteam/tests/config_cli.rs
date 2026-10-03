//! `tagteam config list|get|path` through the binary (§6.4), and `default_provider` (§13.1).
//! Needs `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::cmd;
use serde_json::{Value, json};

/// Writes `text` as the `config.toml` of `root`'s fixture HOME, and returns its path.
fn write_config(root: &Path, text: &str) -> PathBuf {
    let dir = root.join("home/.config/tagteam");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    fs::write(&path, text).unwrap();
    path
}

/// A successful command's stdout and stderr.
fn ok(root: &Path, args: &[&str]) -> (String, String) {
    let out = cmd(root).args(args).assert().success().get_output().clone();
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// A successful `--json` command's object; it warns about nothing.
fn ok_json(root: &Path, args: &[&str]) -> Value {
    let (out, err) = ok(root, args);
    assert_eq!(err, "", "{args:?}");
    serde_json::from_str(&out).unwrap()
}

#[test]
fn the_config_reads_create_nothing_and_never_ask_the_keychain() {
    // §5: a command that changes nothing creates nothing. A locked keychain refuses none of
    // them: `config` touches no Keychain item, so it runs no lock check.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    fs::create_dir_all(d.path().join("keychain")).unwrap();
    fs::write(d.path().join("keychain/LOCKED"), "").unwrap();
    let cases: [&[&str]; 6] = [
        &["config", "list"],
        &["config", "list", "--json"],
        &["config", "get", "autoswitch.threshold"],
        &[
            "config",
            "get",
            "provider.claude-code.autoswitch.models",
            "--json",
        ],
        &["config", "path"],
        &["config", "path", "--json"],
    ];
    for args in cases {
        cmd(d.path()).args(args).assert().success().stderr("");
    }
    assert!(
        fs::read_dir(d.path().join("home"))
            .unwrap()
            .next()
            .is_none(),
        "HOME must stay empty"
    );
    let keychain: Vec<_> = fs::read_dir(d.path().join("keychain"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(keychain, ["LOCKED"]);
}

#[test]
fn config_path_prints_the_path_whether_or_not_the_file_exists() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("home/.config/tagteam/config.toml");
    let (out, _) = ok(d.path(), &["config", "path"]);
    assert_eq!(out, format!("{}\n", path.display()));
    assert_eq!(
        ok_json(d.path(), &["config", "path", "--json"]),
        json!({"schemaVersion": 1, "path": path.display().to_string(), "exists": false})
    );
    write_config(d.path(), "");
    assert_eq!(
        ok_json(d.path(), &["config", "path", "--json"])["exists"],
        true
    );
    // An absolute XDG_CONFIG_HOME moves it (§5).
    let xdg = d.path().join("xdg");
    let out = cmd(d.path())
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["config", "path"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        format!("{}\n", xdg.join("tagteam/config.toml").display())
    );
}

/// A file with a provider override, a global value and a key the registry does not know.
const MIXED: &str = "[autoswitch]\nthreshold = 80\nmodels = [\"Opus\"]\nthresold = 1\n\n\
                     [provider.claude-code.autoswitch]\nmodels = []\n";

#[test]
fn config_list_shows_every_key_with_its_value_and_source_then_the_unknown_ones() {
    let d = tempfile::tempdir().unwrap();
    write_config(d.path(), MIXED);
    let (out, err) = ok(d.path(), &["config", "list"]);
    assert_eq!(err, "", "an unknown key is listed, not warned about");
    assert_eq!(
        out,
        concat!(
            "KEY                                  SOURCE    VALUE\n",
            "default_provider                     default   claude-code\n",
            "autoswitch.threshold                 global    80\n",
            "autoswitch.interval_seconds          default   60\n",
            "autoswitch.cooldown_seconds          default   300\n",
            "autoswitch.hysteresis_pct            default   10\n",
            "autoswitch.strategy                  default   best\n",
            "autoswitch.include_api_key_accounts  default   false\n",
            "autoswitch.unhealthy_ticks           default   3\n",
            "autoswitch.models                    provider  ''\n",
            "usage.history_retention_days         default   180\n",
            "statusline.format                    default   {account} · 5h {5h}% · 7d {7d}%{stale}\n",
            "run.share_extra                      default   ''\n",
            "ui.color                             default   auto\n",
            "\n",
            "Unknown keys, ignored:\n",
            "  autoswitch.thresold\n",
        )
    );
}

#[test]
fn config_list_json_is_the_specs_shape() {
    let d = tempfile::tempdir().unwrap();
    let path = write_config(d.path(), MIXED);
    let row = |key: &str, value: Value, default: Value, source: &str| json!({"key": key, "value": value, "default": default, "source": source});
    let format = json!("{account} · 5h {5h}% · 7d {7d}%{stale}");
    assert_eq!(
        ok_json(d.path(), &["config", "list", "--json"]),
        json!({
            "schemaVersion": 1,
            "path": path.display().to_string(),
            "provider": "claude-code",
            "keys": [
                row("default_provider", json!("claude-code"), json!("claude-code"), "default"),
                row("autoswitch.threshold", json!(80.0), json!(90.0), "global"),
                row("autoswitch.interval_seconds", json!(60), json!(60), "default"),
                row("autoswitch.cooldown_seconds", json!(300), json!(300), "default"),
                row("autoswitch.hysteresis_pct", json!(10.0), json!(10.0), "default"),
                row("autoswitch.strategy", json!("best"), json!("best"), "default"),
                row("autoswitch.include_api_key_accounts", json!(false), json!(false), "default"),
                row("autoswitch.unhealthy_ticks", json!(3), json!(3), "default"),
                row("autoswitch.models", json!([]), json!([]), "provider"),
                row("usage.history_retention_days", json!(180), json!(180), "default"),
                row("statusline.format", format.clone(), format, "default"),
                row("run.share_extra", json!([]), json!([]), "default"),
                row("ui.color", json!("auto"), json!("auto"), "default"),
            ],
            "unknown": ["autoswitch.thresold"],
        })
    );
}

#[test]
fn config_get_prints_the_effective_value_alone() {
    let d = tempfile::tempdir().unwrap();
    write_config(
        d.path(),
        "[autoswitch]\nthreshold = 80\nmodels = [\"Fable\", \"Opus\"]\n\
         [provider.claude-code.statusline]\nformat = \"{7d}\"\n",
    );
    for (key, want) in [
        ("autoswitch.threshold", "80\n"),
        ("autoswitch.models", "Fable,Opus\n"),
        ("autoswitch.strategy", "best\n"),
        ("run.share_extra", "\n"),
        ("statusline.format", "{7d}\n"),
        ("default_provider", "claude-code\n"),
    ] {
        assert_eq!(
            ok(d.path(), &["config", "get", key]),
            (want.to_owned(), String::new()),
            "{key}"
        );
    }
    assert_eq!(
        ok_json(d.path(), &["config", "get", "autoswitch.models", "--json"]),
        json!({"schemaVersion": 1, "key": "autoswitch.models", "provider": "claude-code",
               "value": ["Fable", "Opus"], "source": "global"})
    );
    // `provider.<id>.<key>` and `<key> --provider <id>` name the same entry (§6.4).
    let cases: [&[&str]; 3] = [
        &[
            "config",
            "get",
            "provider.claude-code.statusline.format",
            "--json",
        ],
        &[
            "config",
            "get",
            "statusline.format",
            "--provider",
            "claude-code",
            "--json",
        ],
        &["config", "get", "statusline.format", "--json"],
    ];
    for args in cases {
        assert_eq!(
            ok_json(d.path(), args),
            json!({"schemaVersion": 1, "key": "statusline.format", "provider": "claude-code",
                   "value": "{7d}", "source": "provider"}),
            "{args:?}"
        );
    }
}

#[test]
fn a_key_config_cannot_name_is_invalid_input_not_a_usage_error() {
    // Decision 14: the KEY parser takes any string, so the registry's refusal is the one
    // reported, with the engine's kind.
    const NOT_PER_PROVIDER: &str = "`ui.color` is the same for every provider, so no provider table can set it; name it without `provider.<id>.` and without --provider";
    let d = tempfile::tempdir().unwrap();
    let cases: [(&[&str], &str); 4] = [
        (
            &["config", "get", "autoswitch.thresold"],
            "there is no setting `autoswitch.thresold`; `tagteam config list` shows them all",
        ),
        (
            &["config", "get", "provider.claude-code.ui.color"],
            NOT_PER_PROVIDER,
        ),
        (
            &["config", "get", "ui.color", "--provider", "claude-code"],
            NOT_PER_PROVIDER,
        ),
        (
            &[
                "config",
                "get",
                "provider.fake-agent.autoswitch.threshold",
                "--provider",
                "claude-code",
            ],
            "`provider.fake-agent.…` and `--provider claude-code` name different providers; give only one of them",
        ),
    ];
    for (args, message) in cases {
        cmd(d.path())
            .args(args)
            .assert()
            .code(1)
            .stdout("")
            .stderr(format!("tagteam: {message}\n"));
        let out = cmd(d.path())
            .args(args)
            .arg("--json")
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json!({"schemaVersion": 1, "error": {"type": "invalid-input", "message": message}}),
            "{args:?}"
        );
    }
    // A provider this build lacks is refused alike in both spellings, as `--provider` always was.
    let cases: [(&[&str], &str); 3] = [
        (
            &[
                "config",
                "get",
                "provider.fake-agent.autoswitch.threshold",
                "--json",
            ],
            "fake-agent",
        ),
        (
            &[
                "config",
                "get",
                "autoswitch.threshold",
                "--provider",
                "fake-agent",
                "--json",
            ],
            "fake-agent",
        ),
        (
            &[
                "config",
                "get",
                "provider.Claude.autoswitch.threshold",
                "--json",
            ],
            "Claude",
        ),
    ];
    for (args, provider) in cases {
        let out = cmd(d.path())
            .args(args)
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json!({"schemaVersion": 1, "error": {"type": "unknown-provider",
                   "message": format!("unknown provider {provider:?}")}}),
            "{args:?}"
        );
    }
}

#[test]
fn settings_warnings_go_to_stderr_and_config_still_answers() {
    let d = tempfile::tempdir().unwrap();
    let path = write_config(d.path(), "[autoswitch\n");
    let warning = format!(
        "warning: {}: the settings file is not valid TOML; using the defaults\n",
        path.display()
    );
    let (out, err) = ok(d.path(), &["config", "get", "autoswitch.threshold"]);
    assert_eq!((out.as_str(), err.as_str()), ("90\n", warning.as_str()));
    let (out, err) = ok(d.path(), &["config", "list", "--json"]);
    assert_eq!(err, warning);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(
        v["keys"]
            .as_array()
            .unwrap()
            .iter()
            .all(|k| k["source"] == "default"),
        "{v}"
    );
}

#[test]
fn config_answers_inside_a_run_shell() {
    // §6.4: settings are not accounts, so a run shell (§12.8) refuses none of `config`.
    let d = tempfile::tempdir().unwrap();
    let (_profile, shell) = common::cc_profile(d.path(), "0192-not-managed");
    let path = write_config(d.path(), "[autoswitch]\nthreshold = 80\n");
    let inside = |args: &[&str]| -> String {
        let out = cmd(d.path())
            .env("CLAUDE_CONFIG_DIR", &shell)
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        String::from_utf8(out).unwrap()
    };
    assert_eq!(inside(&["config", "get", "autoswitch.threshold"]), "80\n");
    assert_eq!(inside(&["config", "path"]), format!("{}\n", path.display()));
    assert!(inside(&["config", "list"]).starts_with("KEY "));
}
