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

#[test]
fn a_default_provider_this_build_lacks_warns_and_claude_code_is_used() {
    // §13.1 rule 3 and Decision 2: the setting reads as written, and the CLI falls back.
    let d = tempfile::tempdir().unwrap();
    let path = write_config(d.path(), "default_provider = \"fake-agent\"\n");
    let warning = format!(
        "warning: {}: `default_provider` names fake-agent, which this build does not have; using claude-code\n",
        path.display()
    );
    let (out, err) = ok(d.path(), &["config", "get", "default_provider"]);
    assert_eq!(
        (out.as_str(), err.as_str()),
        ("fake-agent\n", warning.as_str())
    );
    let (out, err) = ok(d.path(), &["config", "list", "--json"]);
    assert_eq!(err, warning);
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["provider"],
        "claude-code"
    );
    let (out, err) = ok(d.path(), &["status", "--json"]);
    assert_eq!(err, warning);
    assert_eq!(
        out,
        "{\"schemaVersion\":1,\"provider\":\"claude-code\",\"active\":null}\n"
    );
    // The status bar has nowhere to show the warning, and falls back alike (§13.5).
    cmd(d.path())
        .arg("statusline")
        .assert()
        .success()
        .stdout("")
        .stderr("");
}

/// `config set` and `config unset` through the real binary (§6.4).
mod set_and_unset {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::process::Stdio;

    use serde_json::{Value, json};
    use tagteam_engine::settings::config_path;
    use tagteam_provider::Env;

    use super::common::{cmd, std_cmd};

    /// The settings file of the binary's environment under `root`.
    fn settings_file(root: &Path) -> PathBuf {
        config_path(&Env::for_test(root))
    }

    /// The exit code of `args --json`, and the one object it printed.
    fn run_json(root: &Path, args: &[&str]) -> (i32, Value) {
        let out = cmd(root).args(args).arg("--json").output().unwrap();
        (
            out.status.code().unwrap(),
            serde_json::from_slice(&out.stdout).unwrap(),
        )
    }

    #[test]
    fn set_and_unset_answer_with_the_spec_s_json_shape() {
        // §6.4: `{schemaVersion, ok, key, value, changed}`, with `value` null for `unset`.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let models = "provider.claude-code.autoswitch.models";

        assert_eq!(
            run_json(root, &["config", "set", models, "Fable, opus"]),
            (
                0,
                json!({"schemaVersion": 1, "ok": true, "key": models, "value": ["Fable", "opus"], "changed": true})
            )
        );
        // `--provider` names the same entry, which already holds the list.
        assert_eq!(
            run_json(
                root,
                &[
                    "config",
                    "set",
                    "autoswitch.models",
                    "Fable,opus",
                    "--provider",
                    "claude-code"
                ]
            ),
            (
                0,
                json!({"schemaVersion": 1, "ok": true, "key": models, "value": ["Fable", "opus"], "changed": false})
            )
        );
        assert_eq!(
            run_json(root, &["config", "set", "autoswitch.threshold", "85.5"]),
            (
                0,
                json!({"schemaVersion": 1, "ok": true, "key": "autoswitch.threshold", "value": 85.5, "changed": true})
            )
        );
        for changed in [true, false] {
            assert_eq!(
                run_json(
                    root,
                    &["config", "unset", "autoswitch.models", "-p", "claude-code"]
                ),
                (
                    0,
                    json!({"schemaVersion": 1, "ok": true, "key": models, "value": null, "changed": changed})
                )
            );
        }
        assert_eq!(
            fs::read_to_string(settings_file(root)).unwrap(),
            "[autoswitch]\nthreshold = 85.5\n"
        );
    }

    #[test]
    fn a_refused_value_or_key_exits_1_with_invalid_input_and_writes_nothing() {
        // §6.4, §13.1. `ConfigKeyParser` passes any key, so an unknown key reaches the engine:
        // exit 1, not clap's 2 (Decision 14). A value that starts with `-` reaches the engine
        // too.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        for args in [
            &["config", "set", "autoswitch.threshold", "100"][..],
            &["config", "set", "autoswitch.cooldown_seconds", "-1"],
            &["config", "set", "autoswitch.treshold", "80"],
            &["config", "set", "provider.claude-code.ui.color", "never"],
            &[
                "config",
                "set",
                "ui.color",
                "never",
                "--provider",
                "claude-code",
            ],
            &[
                "config",
                "set",
                "provider.claude-code.autoswitch.models",
                "all,Fable",
            ],
            &["config", "set", "default_provider", "codex"],
            &["config", "unset", "autoswitch.treshold"],
        ] {
            let (code, v) = run_json(root, args);
            assert_eq!(code, 1, "{args:?}");
            assert_eq!(v["schemaVersion"], 1, "{args:?}");
            assert_eq!(v["error"]["type"], "invalid-input", "{args:?}: {v}");
        }
        // A provider this build lacks is `unknown-provider` in both spellings, as for `get`
        // (Decision 4).
        for args in [
            &["config", "set", "provider.codex.autoswitch.threshold", "80"][..],
            &[
                "config",
                "set",
                "autoswitch.threshold",
                "80",
                "--provider",
                "codex",
            ],
            &["config", "unset", "provider.codex.autoswitch.threshold"],
        ] {
            let (code, v) = run_json(root, args);
            assert_eq!(code, 1, "{args:?}");
            assert_eq!(v["error"]["type"], "unknown-provider", "{args:?}: {v}");
        }
        // A missing VALUE is a usage error.
        cmd(root)
            .args(["config", "set", "ui.color"])
            .assert()
            .code(2);
        // Without `--json`, the refusal is one line on stderr.
        cmd(root)
            .args(["config", "set", "autoswitch.threshold", "100"])
            .assert()
            .code(1)
            .stdout("")
            .stderr(predicates::str::starts_with("tagteam: "));
        assert!(!settings_file(root).exists());
    }

    #[test]
    fn set_and_unset_say_what_they_changed() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let file = settings_file(root).display().to_string();
        let says = |args: &[&str], text: String| {
            cmd(root)
                .args(args)
                .assert()
                .success()
                .stdout(text)
                .stderr("");
        };

        says(
            &["config", "set", "ui.color", "never"],
            format!("Set ui.color = \"never\" in {file}.\n"),
        );
        says(
            &["config", "set", "ui.color", "never"],
            "ui.color is already \"never\".\n".into(),
        );
        says(
            &[
                "config",
                "set",
                "provider.claude-code.autoswitch.models",
                "",
            ],
            format!("Set provider.claude-code.autoswitch.models = [] in {file}.\n"),
        );
        says(
            &["config", "unset", "ui.color"],
            format!("Removed ui.color from {file}.\n"),
        );
        says(
            &["config", "unset", "ui.color"],
            "ui.color is not set.\n".into(),
        );
    }

    #[test]
    fn a_corrupt_file_is_refused_with_settings_unreadable_and_left_as_it_was() {
        // §6.4: `set` and `unset` refuse to write to a corrupt file.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let file = settings_file(root);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "[ui\ncolor = \"never\"\n").unwrap();
        for args in [
            &["config", "set", "ui.color", "auto"][..],
            &["config", "unset", "ui.color"],
        ] {
            let (code, v) = run_json(root, args);
            assert_eq!(code, 1, "{args:?}");
            assert_eq!(v["error"]["type"], "settings-unreadable", "{args:?}: {v}");
        }
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "[ui\ncolor = \"never\"\n"
        );
    }

    #[test]
    fn a_symlinked_settings_file_is_written_through_and_stays_a_link() {
        // Review Focus 1, through the binary, with an absolute link (the engine test uses a
        // relative one).
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let target = root.join("dotfiles/tagteam.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(
            &target,
            "# from my dotfiles\n[ui]\ncolor = \"auto\" # for now\n",
        )
        .unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let link = settings_file(root);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&target, &link).unwrap();

        cmd(root)
            .args(["config", "set", "ui.color", "never"])
            .assert()
            .success();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "# from my dotfiles\n[ui]\ncolor = \"never\" # for now\n"
        );
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
            0o644
        );
    }

    #[test]
    fn processes_setting_different_keys_at_once_all_land() {
        // §6.4: the settings lock orders each read, edit and write across processes, so no
        // process writes over another's key.
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let writes = [
            ("autoswitch.threshold", "85.5", "threshold = 85.5"),
            (
                "autoswitch.interval_seconds",
                "120",
                "interval_seconds = 120",
            ),
            (
                "autoswitch.cooldown_seconds",
                "600",
                "cooldown_seconds = 600",
            ),
            ("autoswitch.unhealthy_ticks", "4", "unhealthy_ticks = 4"),
            (
                "usage.history_retention_days",
                "30",
                "history_retention_days = 30",
            ),
            ("ui.color", "never", "color = \"never\""),
        ];
        let children: Vec<_> = writes
            .iter()
            .map(|&(key, value, _)| {
                std_cmd(root)
                    .args(["config", "set", key, value])
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for child in children {
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let text = fs::read_to_string(settings_file(root)).unwrap();
        for (key, _, line) in writes {
            assert!(text.lines().any(|l| l == line), "{key} was lost:\n{text}");
        }
    }
}
