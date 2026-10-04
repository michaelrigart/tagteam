//! `config set` and `unset` (§6.4): strict edits of one entry of `config.toml` that keep every
//! comment, write through a symlink and hold the settings lock.

mod common;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::sync::Barrier;
use std::thread;
use std::time::Instant;

use common::{FakeFx, Fx, capture_logs, cc};
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EngineError;
use tagteam_engine::config::{SETTINGS_LOCK_TIMEOUT, SettingsChange};
use tagteam_engine::settings::{
    ColorMode, KEYS, KeyKind, Settings, SettingsError, Value, config_path,
};
use tagteam_provider::FlockGuard;
use tagteam_provider::profile::RunShell;

/// Writes `text` as the fixture's settings file, making its directory first.
fn write_config(fx: &Fx, text: &str) {
    let path = config_path(&fx.env);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// The fixture's settings file.
fn config_text(fx: &Fx) -> String {
    fs::read_to_string(config_path(&fx.env)).unwrap()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn set(fx: &Fx, name: &str, raw: &str) -> SettingsChange {
    fx.engine
        .config_set(name, None, raw)
        .unwrap_or_else(|e| panic!("config set {name} {raw:?}: {e}"))
}

fn unset(fx: &Fx, name: &str) -> SettingsChange {
    fx.engine
        .config_unset(name, None)
        .unwrap_or_else(|e| panic!("config unset {name}: {e}"))
}

/// The `error.type` of a `config set` that must refuse.
fn refusal(fx: &Fx, name: &str, provider: Option<&ProviderId>, raw: &str) -> &'static str {
    fx.engine
        .config_set(name, provider, raw)
        .unwrap_err()
        .kind()
}

fn list(names: &[&str]) -> Value {
    Value::List(names.iter().map(|n| n.to_string()).collect())
}

/// A file as a person keeps it: comments above tables and keys and at the ends of lines, and
/// spacing tagteam would not write itself.
const COMMENTED: &str = "\
# tagteam settings, kept in my dotfiles

[autoswitch]
# 85 leaves room for a long session
threshold = 80.0 # was 90
models = [\"Fable\"]   # the one that runs out

# colours off in CI
[ui]
color = \"never\"
";

#[test]
fn a_set_rewrites_its_value_alone_and_every_comment_survives() {
    // §6.4: `set` writes only the key it is given, through `toml_edit`. Byte for byte, the rest
    // of the file is what the person wrote, and an override added then removed leaves no trace.
    let fx = Fx::new();
    write_config(&fx, COMMENTED);

    set(&fx, "autoswitch.threshold", "85.5");
    let updated = COMMENTED.replace("threshold = 80.0 # was 90", "threshold = 85.5 # was 90");
    assert_eq!(config_text(&fx), updated);

    set(&fx, "provider.claude-code.autoswitch.models", "");
    assert_eq!(
        config_text(&fx),
        format!("{updated}\n[provider.claude-code.autoswitch]\nmodels = []\n")
    );

    unset(&fx, "provider.claude-code.autoswitch.models");
    assert_eq!(config_text(&fx), updated, "the emptied tables went with it");
}

#[test]
fn set_creates_the_file_0600_in_a_0700_directory_and_reports_the_entry() {
    // §5, §6.4: only `set` creates the file. A new file is 0600 whatever the umask, in a
    // directory made 0700.
    let fx = Fx::new();
    let path = config_path(&fx.env);
    assert!(!fx.env.config_dir().exists());

    let change = set(&fx, "ui.color", "never");

    assert_eq!(
        change,
        SettingsChange {
            key: "ui.color".into(),
            value: Some(Value::Str("never".into())),
            changed: true,
            path: path.clone(),
        }
    );
    assert_eq!(config_text(&fx), "[ui]\ncolor = \"never\"\n");
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&fx.env.config_dir()), 0o700);
}

#[test]
fn an_existing_file_keeps_its_mode() {
    // §6.4: "an existing file keeps its mode".
    let fx = Fx::new();
    write_config(&fx, "[ui]\ncolor = \"auto\"\n");
    let path = config_path(&fx.env);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    set(&fx, "ui.color", "never");

    assert_eq!(config_text(&fx), "[ui]\ncolor = \"never\"\n");
    assert_eq!(mode(&path), 0o644);
}

#[test]
fn a_symlinked_file_is_written_through_to_its_target_and_stays_a_link() {
    // Review Focus 1: `config.toml` as chezmoi or a dotfiles repo leaves it, here a relative
    // link into the repo. The write lands in the target with its comments, the link stays a
    // link, the target keeps its mode, and neither directory keeps a temporary file.
    let fx = Fx::new();
    let dotfiles = fx.dir.path().join("dotfiles");
    fs::create_dir_all(&dotfiles).unwrap();
    let target = dotfiles.join("tagteam.toml");
    fs::write(
        &target,
        "# shared across my machines\n[ui]\ncolor = \"auto\" # for now\n",
    )
    .unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    let link = config_path(&fx.env);
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    // `<root>/home/.config/tagteam/config.toml` → `<root>/dotfiles/tagteam.toml`
    symlink("../../../dotfiles/tagteam.toml", &link).unwrap();

    let change = set(&fx, "ui.color", "never");

    assert!(change.changed);
    assert_eq!(change.path, link);
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(&link).unwrap(),
        Path::new("../../../dotfiles/tagteam.toml")
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "# shared across my machines\n[ui]\ncolor = \"never\" # for now\n"
    );
    assert_eq!(mode(&target), 0o640);
    assert_eq!(fs::read_dir(&dotfiles).unwrap().count(), 1);
    assert_eq!(fs::read_dir(link.parent().unwrap()).unwrap().count(), 1);

    unset(&fx, "ui.color");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "# shared across my machines\n[ui]\n",
        "the emptied table keeps the comment above its header"
    );
}

#[test]
fn an_empty_list_override_beats_a_non_empty_global_list() {
    // Review Focus 2: `''` writes `[]` into the provider's own table, which the reader takes
    // over the global list. Another provider still reads the global list.
    let fx = Fx::new();
    write_config(&fx, "[autoswitch]\nmodels = [\"Fable\"]\n");

    let change = set(&fx, "provider.claude-code.autoswitch.models", "");

    assert_eq!(change.value, Some(list(&[])));
    assert_eq!(
        config_text(&fx),
        "[autoswitch]\nmodels = [\"Fable\"]\n\n[provider.claude-code.autoswitch]\nmodels = []\n"
    );
    let (settings, warnings) = Settings::load(&fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(settings.models.is_empty());
    let (other, _) = Settings::load(&fx.env, &ProviderId::new("fake-agent"));
    assert_eq!(other.models, ["Fable"]);
}

#[test]
fn lists_are_typed_comma_separated_with_each_name_trimmed() {
    // Review Focus 2, §6.4 "Values on the command line": names typed with a space are trimmed
    // names. An empty item refuses, and so does `all` beside a name. A refusal writes nothing.
    let fx = Fx::new();
    for raw in ["Fable,,Opus", "all,Fable", "Fable,"] {
        assert_eq!(
            refusal(&fx, "autoswitch.models", None, raw),
            "invalid-input",
            "{raw:?}"
        );
    }
    assert!(!config_path(&fx.env).exists());

    let change = set(&fx, "autoswitch.models", "Fable, opus");
    assert_eq!(change.value, Some(list(&["Fable", "opus"])));
    assert_eq!(
        config_text(&fx),
        "[autoswitch]\nmodels = [\"Fable\", \"opus\"]\n"
    );
    assert_eq!(
        set(&fx, "autoswitch.models", "all").value,
        Some(list(&["all"]))
    );
}

#[test]
fn a_negative_zero_is_stored_and_shown_as_zero() {
    // `-0` is in 0-50's range, and as a float it keeps its sign: `-0.0` in the file and `-0` in
    // `config get`. A zero is stored as a plain zero.
    let fx = Fx::new();
    for raw in ["-0", "-0.0"] {
        let change = set(&fx, "autoswitch.hysteresis_pct", raw);
        let Some(Value::Float(n)) = change.value else {
            panic!("{raw:?}: {:?}", change.value);
        };
        assert!(n == 0.0 && n.is_sign_positive(), "{raw:?} gave {n:?}");
        assert_eq!(Value::Float(n).display(), "0");
        let text = config_text(&fx);
        assert!(!text.contains("-0"), "{raw:?} wrote {text}");
    }
}

#[test]
fn the_spec_s_ranges_and_spellings_hold_at_both_ends_and_nothing_is_clamped() {
    // §6.4's table, end to end. Each bound is taken as typed, and one step past it refuses
    // with `invalid-input`. A refusal writes nothing, so a value is never clamped.
    let fx = Fx::new();
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "autoswitch.threshold",
            &["50", "99.9"],
            &["49.9", "99.95", "high"],
        ),
        (
            "autoswitch.interval_seconds",
            &["15", "3600"],
            &["14", "3601", "60.5"],
        ),
        (
            "autoswitch.cooldown_seconds",
            &["0", "86400"],
            &["-1", "86401"],
        ),
        ("autoswitch.hysteresis_pct", &["0", "50"], &["-0.1", "50.1"]),
        ("autoswitch.unhealthy_ticks", &["1", "100"], &["0", "101"]),
        (
            "usage.history_retention_days",
            &["1", "3650"],
            &["0", "3651"],
        ),
        (
            "autoswitch.strategy",
            &["best", "consume-first"],
            &["fastest", ""],
        ),
        (
            "autoswitch.include_api_key_accounts",
            &["yes", "0"],
            &["on", "2", ""],
        ),
        ("ui.color", &["auto", "always", "never"], &["sometimes"]),
        (
            "statusline.format",
            &["{account} {5h}%", "{model:Fable}"],
            &["", "{bogus}", "{5h"],
        ),
        ("run.share_extra", &["hook-data", ""], &["a/b", ".."]),
    ];
    for (name, taken, refused) in cases {
        for raw in *refused {
            let before = fs::read(config_path(&fx.env)).ok();
            assert_eq!(
                refusal(&fx, name, None, raw),
                "invalid-input",
                "{name} {raw:?}"
            );
            assert_eq!(
                fs::read(config_path(&fx.env)).ok(),
                before,
                "{name} {raw:?} wrote"
            );
        }
        for raw in *taken {
            assert!(set(&fx, name, raw).changed, "{name} {raw:?}");
        }
    }
    let (settings, warnings) = Settings::load(&fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(settings.threshold, 99.9);
    assert_eq!(settings.color, ColorMode::Never);
}

#[test]
fn every_numeric_key_in_the_registry_refuses_a_step_past_either_end() {
    // §6.4: `set` never clamps. Each range comes from the registry, so a key added to it is
    // covered without a new case here.
    let fx = Fx::new();
    for key in KEYS {
        let outside = match key.kind {
            KeyKind::Float { min, max } => [min - 0.01, max + 0.01].map(|v| v.to_string()),
            KeyKind::Int { min, max } => [min - 1, max + 1].map(|v| v.to_string()),
            _ => continue,
        };
        for raw in outside {
            assert_eq!(
                refusal(&fx, key.name, None, &raw),
                "invalid-input",
                "{} {raw}",
                key.name
            );
        }
    }
    assert!(
        !config_path(&fx.env).exists(),
        "nothing was written, clamped or otherwise"
    );
}

/// A value `set` takes for a key of `kind`: the bottom of a range, or the last choice.
fn sample(kind: &KeyKind) -> String {
    match kind {
        KeyKind::Float { min, .. } => min.to_string(),
        KeyKind::Int { min, .. } => min.to_string(),
        KeyKind::Bool => "yes".into(),
        KeyKind::Choice(choices) => choices.last().unwrap().to_string(),
        KeyKind::Models => "Fable, opus".into(),
        KeyKind::Format => "{account} · {7d}%".into(),
        KeyKind::ShareNames => "hook-data".into(),
        KeyKind::Provider => cc().to_string(),
    }
}

#[test]
fn every_key_set_through_the_registry_reads_back_without_a_warning() {
    // §6.4 "One registry": the forgiving reader takes whatever `set` writes as it is, from the
    // global table and from a provider's.
    let fx = Fx::new();
    for key in KEYS {
        let raw = sample(&key.kind);
        set(&fx, key.name, &raw);
        if key.per_provider {
            fx.engine
                .config_set(key.name, Some(&cc()), &raw)
                .unwrap_or_else(|e| panic!("{} --provider: {e}", key.name));
        }
    }
    let (_, warnings) = Settings::load(&fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn an_unknown_key_refuses_for_set_and_unset() {
    // §6.4, Decision 14: the CLI passes any key through, and the engine refuses one the
    // registry lacks with `invalid-input`. A typo is reported, never "not set".
    let fx = Fx::new();
    write_config(&fx, "[autoswitch]\ntreshold = 80\n");
    for name in [
        "autoswitch.treshold",
        "provider.claude-code.autoswitch.treshold",
        "threshold",
        "",
    ] {
        assert_eq!(refusal(&fx, name, None, "80"), "invalid-input", "{name:?}");
        assert_eq!(
            fx.engine.config_unset(name, None).unwrap_err().kind(),
            "invalid-input",
            "{name:?}"
        );
    }
    assert_eq!(config_text(&fx), "[autoswitch]\ntreshold = 80\n");
}

#[test]
fn a_key_no_provider_table_may_hold_refuses_under_either_spelling() {
    // §6.4: `provider.<id>.ui.color` and `ui.color --provider <id>` name the same entry, which
    // no provider table may hold. `set` and `unset` both refuse it.
    let fx = Fx::new();
    for (name, raw) in [
        ("ui.color", "never"),
        ("usage.history_retention_days", "30"),
        ("default_provider", "claude-code"),
    ] {
        let prefixed = format!("provider.claude-code.{name}");
        assert_eq!(
            refusal(&fx, &prefixed, None, raw),
            "invalid-input",
            "{prefixed}"
        );
        assert_eq!(
            refusal(&fx, name, Some(&cc()), raw),
            "invalid-input",
            "{name} --provider"
        );
        assert_eq!(
            fx.engine.config_unset(&prefixed, None).unwrap_err().kind(),
            "invalid-input"
        );
        assert_eq!(
            fx.engine
                .config_unset(name, Some(&cc()))
                .unwrap_err()
                .kind(),
            "invalid-input"
        );
    }
    assert!(!config_path(&fx.env).exists());
}

#[test]
fn a_prefix_and_a_flag_naming_two_providers_refuse_and_naming_one_are_one_entry() {
    // §6.4: a prefix and `--provider` that disagree refuse, and nothing is written. When they
    // agree, or only one is given, they all name the one entry.
    let ffx = FakeFx::new();
    let fake = ffx.fake_provider();
    let engine = &ffx.engine;
    assert_eq!(
        engine
            .config_set(
                "provider.claude-code.autoswitch.threshold",
                Some(&fake),
                "80"
            )
            .unwrap_err()
            .kind(),
        "invalid-input"
    );
    assert!(!config_path(&ffx.fx.env).exists());

    let by_prefix = engine
        .config_set("provider.fake-agent.autoswitch.threshold", None, "80")
        .unwrap();
    let by_flag = engine
        .config_set("autoswitch.threshold", Some(&fake), "80")
        .unwrap();
    let by_both = engine
        .config_set(
            "provider.fake-agent.autoswitch.threshold",
            Some(&fake),
            "80",
        )
        .unwrap();

    assert_eq!(by_prefix.key, "provider.fake-agent.autoswitch.threshold");
    assert!(by_prefix.changed);
    assert_eq!(
        by_flag,
        SettingsChange {
            changed: false,
            ..by_prefix.clone()
        }
    );
    assert_eq!(by_both, by_flag);
    assert_eq!(
        config_text(&ffx.fx),
        "[provider.fake-agent.autoswitch]\nthreshold = 80.0\n"
    );
}

#[test]
fn a_provider_this_engine_does_not_register_refuses() {
    // Decision 4: a provider this build lacks, named by a prefix or by `--provider`, is
    // `unknown-provider` for `set` and `unset` alike, as for `config get`. As
    // `default_provider`'s value it is `invalid-input`. `fake-agent` is well formed, but this
    // fixture registers Claude Code alone.
    let fx = Fx::new();
    let fake = ProviderId::new("fake-agent");
    for (name, flag) in [
        ("provider.fake-agent.autoswitch.threshold", None),
        ("autoswitch.threshold", Some(&fake)),
        ("provider.fake-agent.autoswitch.threshold", Some(&fake)),
    ] {
        assert!(
            matches!(
                fx.engine.config_set(name, flag, "80"),
                Err(EngineError::UnknownProvider(id)) if id == "fake-agent"
            ),
            "{name} {flag:?}"
        );
        assert_eq!(
            fx.engine.config_unset(name, flag).unwrap_err().kind(),
            "unknown-provider",
            "{name} {flag:?}"
        );
    }
    assert!(matches!(
        fx.engine.config_set("default_provider", None, "fake-agent"),
        Err(EngineError::Settings(SettingsError::UnknownProvider(id))) if id == "fake-agent"
    ));
    assert_eq!(
        refusal(&fx, "default_provider", None, "fake-agent"),
        "invalid-input"
    );
    assert!(!config_path(&fx.env).exists());
}

#[test]
fn default_provider_takes_a_registered_provider_and_lands_above_every_table() {
    // Decision 2: `default_provider` is a top-level key, so it goes before the first table
    // header, never into a table. toml_edit keeps a comment above a header as that table's
    // own, so the new key goes above such a comment too.
    let ffx = FakeFx::new();
    write_config(
        &ffx.fx,
        "# my tagteam settings\n\n[autoswitch]\nthreshold = 80\n",
    );

    let change = ffx
        .engine
        .config_set("default_provider", None, "fake-agent")
        .unwrap();

    assert_eq!(change.value, Some(Value::Str("fake-agent".into())));
    assert_eq!(
        config_text(&ffx.fx),
        "default_provider = \"fake-agent\"\n# my tagteam settings\n\n[autoswitch]\nthreshold = 80\n"
    );
    ffx.engine
        .config_set("default_provider", None, "claude-code")
        .unwrap();
    assert_eq!(
        config_text(&ffx.fx),
        "default_provider = \"claude-code\"\n# my tagteam settings\n\n[autoswitch]\nthreshold = 80\n",
        "updated where it stands"
    );

    // After other top-level keys, and still above the first header.
    let fx = Fx::new();
    write_config(&fx, "autoswitch.threshold = 80\n\n[ui]\ncolor = \"auto\"\n");
    set(&fx, "default_provider", "claude-code");
    assert_eq!(
        config_text(&fx),
        "autoswitch.threshold = 80\ndefault_provider = \"claude-code\"\n\n[ui]\ncolor = \"auto\"\n"
    );
}

#[test]
fn run_share_extra_refuses_a_name_each_profile_keeps_private() {
    // §6.4 "Key-specific rules", §12.2: `set` refuses a known-private name, matched exactly or
    // by pattern, and tagteam's own `.tagteam-*`, which a read only ignores with a warning.
    let fx = Fx::new();
    for raw in [
        ".credentials.json",
        "hook-data, daemon.log",
        "x.lock",
        ".claude-staging-oauth.json",
        "policy-limits.json",
        ".tagteam-links.json",
    ] {
        assert_eq!(
            refusal(&fx, "run.share_extra", None, raw),
            "invalid-input",
            "{raw}"
        );
        assert_eq!(
            refusal(&fx, "provider.claude-code.run.share_extra", None, raw),
            "invalid-input",
            "{raw}"
        );
    }
    assert!(!config_path(&fx.env).exists());
    assert!(matches!(
        fx.engine.config_set("run.share_extra", None, "hook-data, .credentials.json"),
        Err(EngineError::Settings(SettingsError::Invalid { key, reason }))
            if key == "run.share_extra" && reason.contains(".credentials.json")
    ));
    assert_eq!(
        set(&fx, "run.share_extra", "hook-data, .my-tool").value,
        Some(list(&["hook-data", ".my-tool"]))
    );
}

#[test]
fn the_global_share_extra_is_checked_against_every_provider_with_sessions() {
    // §6.4: the global `run.share_extra` reaches every provider's profiles, so a name any of
    // them keeps private refuses. A provider's own entry answers to that provider's list alone.
    // FakeAgent keeps `procs` private; Claude Code does not.
    let ffx = FakeFx::new();
    let set_in = |name: &str, raw: &str| ffx.engine.config_set(name, None, raw);

    assert_eq!(
        set_in("run.share_extra", "procs").unwrap_err().kind(),
        "invalid-input"
    );
    assert_eq!(
        set_in("provider.fake-agent.run.share_extra", "procs")
            .unwrap_err()
            .kind(),
        "invalid-input"
    );
    assert!(
        set_in("provider.claude-code.run.share_extra", "procs")
            .unwrap()
            .changed
    );
    assert!(
        set_in("provider.fake-agent.run.share_extra", ".credentials.json")
            .unwrap()
            .changed
    );
    assert_eq!(
        config_text(&ffx.fx),
        "[provider.claude-code.run]\nshare_extra = [\"procs\"]\n\n\
         [provider.fake-agent.run]\nshare_extra = [\".credentials.json\"]\n"
    );
}

#[test]
fn a_corrupt_file_is_refused_for_set_and_unset_and_left_byte_identical() {
    // §6.4: `set` and `unset` refuse to write to a corrupt file, which a read only warns about.
    let fx = Fx::new();
    let path = config_path(&fx.env);
    for (bytes, detail) in [
        (
            &b"[ui]\ncolor = \"never\"\n[autoswitch\n"[..],
            "not valid TOML (line 3",
        ),
        (&b"[ui]\ncolor = \"\xff\"\n"[..], "not UTF-8"),
    ] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        for result in [
            fx.engine.config_set("ui.color", None, "never"),
            fx.engine.config_unset("ui.color", None),
        ] {
            let err = result.unwrap_err();
            assert_eq!(err.kind(), "settings-unreadable", "{err:?}");
            assert!(
                matches!(
                    &err,
                    EngineError::Settings(SettingsError::Corrupt { path: p, detail: d })
                        if *p == path && d.starts_with(detail)
                ),
                "{err:?}"
            );
        }
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn a_change_to_nothing_writes_nothing_and_creates_nothing() {
    // §6.4: `unset` of an absent key changes nothing and succeeds. §5: a command that changes
    // nothing creates nothing: not the file, not its directory, not even the settings lock.
    let fx = Fx::new();
    let quiet = |change: SettingsChange| assert!(!change.changed, "{change:?}");

    quiet(unset(&fx, "ui.color"));
    quiet(unset(&fx, "provider.claude-code.autoswitch.models"));
    assert!(!fx.env.config_dir().exists());

    // The value already there, as a read takes it: an integer threshold, a single model name.
    let text = "[autoswitch]\nthreshold = 85\nmodels = \"Fable\"\n";
    write_config(&fx, text);
    quiet(set(&fx, "autoswitch.threshold", "85"));
    quiet(set(&fx, "autoswitch.models", "Fable"));
    quiet(unset(&fx, "ui.color"));
    quiet(unset(&fx, "provider.claude-code.autoswitch.threshold"));
    assert_eq!(config_text(&fx), text);
    assert!(
        !fx.env.data_dir().join("locks").exists(),
        "no lock was taken"
    );
}

#[test]
fn unset_removes_the_tables_it_empties_unless_one_holds_a_comment() {
    // §6.4: `unset` removes a table it leaves empty, walking up through `provider.<id>`, unless
    // the table holds a comment above its header or on its line. A key's own comments, above
    // it and at the end of its line, go with the key.
    let cases: &[(&str, &str, &str)] = &[
        (
            "[ui]\ncolor = \"auto\"\n\n[provider.claude-code.autoswitch]\nmodels = []\n",
            "provider.claude-code.autoswitch.models",
            "[ui]\ncolor = \"auto\"\n",
        ),
        (
            "# for the work account\n[provider.claude-code.autoswitch]\nmodels = []\n",
            "provider.claude-code.autoswitch.models",
            "# for the work account\n[provider.claude-code.autoswitch]\n",
        ),
        (
            "[autoswitch] # tuned by hand\nthreshold = 80\n",
            "autoswitch.threshold",
            "[autoswitch] # tuned by hand\n",
        ),
        (
            "[provider.claude-code.autoswitch]\nmodels = []\n\n\
             [provider.claude-code.statusline]\nformat = \"{5h}%\"\n",
            "provider.claude-code.autoswitch.models",
            "[provider.claude-code.statusline]\nformat = \"{5h}%\"\n",
        ),
        (
            "[autoswitch]\nthreshold = 80\n\n[ui]\ncolor = \"auto\"\n",
            "autoswitch.threshold",
            "[ui]\ncolor = \"auto\"\n",
        ),
        (
            "[autoswitch]\n# was 95\nthreshold = 80 # for now\ncooldown_seconds = 600\n",
            "autoswitch.threshold",
            "[autoswitch]\ncooldown_seconds = 600\n",
        ),
        (
            "default_provider = \"claude-code\"\n\n[ui]\ncolor = \"auto\"\n",
            "default_provider",
            "[ui]\ncolor = \"auto\"\n",
        ),
        ("[ui]\ncolor = \"auto\"\n", "ui.color", ""),
    ];
    for (before, name, after) in cases {
        let fx = Fx::new();
        write_config(&fx, before);
        assert!(unset(&fx, name).changed, "{name} in {before:?}");
        assert_eq!(config_text(&fx), *after, "{name} in {before:?}");
    }
}

#[test]
fn dotted_keys_already_in_the_file_are_updated_where_they_stand() {
    // A top-level `autoswitch.threshold = 80` is the `[autoswitch]` table written with dotted
    // keys. `set` updates it in place and adds a sibling in the same style.
    let fx = Fx::new();
    write_config(
        &fx,
        "autoswitch.threshold = 80 # mine\n\n[ui]\ncolor = \"auto\"\n",
    );

    set(&fx, "autoswitch.threshold", "85.5");
    set(&fx, "autoswitch.cooldown_seconds", "600");

    assert_eq!(
        config_text(&fx),
        "autoswitch.threshold = 85.5 # mine\nautoswitch.cooldown_seconds = 600\n\n[ui]\ncolor = \"auto\"\n"
    );
    let (settings, warnings) = Settings::load(&fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(settings.threshold, 85.5);
}

#[test]
fn a_segment_or_an_entry_that_is_a_table_refuses_naming_it() {
    // An existing segment that is not a table, or an entry that is a table, would have to be
    // deleted to write the value. `set` refuses instead and leaves the file alone.
    let cases = [
        ("autoswitch = 3\n", "autoswitch.threshold", "`autoswitch`"),
        (
            "[[provider]]\nid = 1\n",
            "provider.claude-code.autoswitch.threshold",
            "`provider`",
        ),
        (
            "[autoswitch.threshold]\nx = 1\n",
            "autoswitch.threshold",
            "a table",
        ),
        (
            "[autoswitch]\nthreshold = { custom = 1 }\n",
            "autoswitch.threshold",
            "a table",
        ),
    ];
    for (text, name, named) in cases {
        let fx = Fx::new();
        write_config(&fx, text);
        match fx.engine.config_set(name, None, "85.5") {
            Err(EngineError::Settings(SettingsError::Invalid { key, reason })) => {
                assert_eq!(key, name);
                assert!(reason.contains(named), "{reason}");
            }
            other => panic!("{name} in {text:?}: {other:?}"),
        }
        assert_eq!(config_text(&fx), text);
    }
}

#[test]
fn an_unset_of_an_entry_that_is_a_table_refuses_and_keeps_what_it_holds() {
    let cases = [
        "[autoswitch.threshold]\nx = 1\n",
        "[autoswitch]\nthreshold = { custom = 1 }\n",
        "[[autoswitch.threshold]]\nx = 1\n",
    ];
    for text in cases {
        let fx = Fx::new();
        write_config(&fx, text);
        match fx.engine.config_unset("autoswitch.threshold", None) {
            Err(EngineError::Settings(SettingsError::Invalid { key, reason })) => {
                assert_eq!(key, "autoswitch.threshold");
                assert!(reason.contains("a table"), "{reason}");
            }
            other => panic!("{text:?}: {other:?}"),
        }
        assert_eq!(config_text(&fx), text);
    }
}

#[test]
fn a_held_settings_lock_times_out_after_five_seconds_with_lock_timeout() {
    // §6.4: the settings lock is waited for up to 5 s. A write that cannot take it writes
    // nothing.
    let fx = Fx::new();
    let held = FlockGuard::try_lock(&fx.env.data_dir().join("locks/config.lock"))
        .unwrap()
        .unwrap();
    let started = Instant::now();

    let err = fx.engine.config_set("ui.color", None, "never").unwrap_err();

    assert!(started.elapsed() >= SETTINGS_LOCK_TIMEOUT);
    assert_eq!(err.kind(), "lock-timeout");
    assert!(!config_path(&fx.env).exists());
    drop(held);
    assert!(set(&fx, "ui.color", "never").changed);
}

#[test]
fn a_signal_ends_the_settings_lock_wait_as_interrupted() {
    // §6.4, §14.1: each wait for the settings lock is a cancellation point. The token is
    // checked before the first attempt, so a signal already recorded takes nothing.
    let fx = Fx::new();
    fx.engine.cancel().request(15); // SIGTERM

    let err = fx.engine.config_set("ui.color", None, "never").unwrap_err();

    assert_eq!(err.kind(), "interrupted");
    assert_eq!(err.signal(), Some(15));
    assert!(!config_path(&fx.env).exists());
}

#[test]
fn two_writers_in_two_engines_at_once_both_land() {
    // §6.4: each read, edit and write happens under the settings lock, so two processes that
    // change different keys at once keep both changes.
    let fx = Fx::new();
    let engines = [
        fx.engine_with_env(fx.env.clone()),
        fx.engine_with_env(fx.env.clone()),
    ];
    let start = Barrier::new(2);
    thread::scope(|s| {
        let writes = [("autoswitch.threshold", "85.5"), ("ui.color", "never")];
        for (engine, (name, raw)) in engines.iter().zip(writes) {
            let start = &start;
            s.spawn(move || {
                start.wait();
                assert!(engine.config_set(name, None, raw).unwrap().changed);
            });
        }
    });
    let (settings, warnings) = Settings::load(&fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        (settings.threshold, settings.color),
        (85.5, ColorMode::Never)
    );
}

#[test]
fn config_writes_inside_a_run_shell() {
    // §6.4: "`config` works inside a run shell (§12.8): settings are not accounts". The run
    // shell writes the same file the outer home does.
    let fx = Fx::new();
    let profile = fx.dir.path().join("profile");
    fx.write_marker(&profile, &AccountId::from_string("0192"), &fx.env);
    let engine = fx.engine_located(fx.shell_env(&profile));
    assert!(matches!(engine.run_shell(), RunShell::Inside { .. }));

    let change = engine.config_set("ui.color", None, "never").unwrap();

    assert!(change.changed);
    assert_eq!(change.path, config_path(&fx.env));
    assert_eq!(config_text(&fx), "[ui]\ncolor = \"never\"\n");
}

#[test]
fn a_write_logs_one_info_line_naming_the_key_and_never_the_value() {
    // §14.2: settings writes are INFO. A value can be free text, which may hold an email or a
    // name the log never records, so the line names the key alone.
    let fx = Fx::new();
    let name = "provider.claude-code.statusline.format";
    let key_field = format!("key={name}");
    // `tracing` caches whether a call site is enabled the first time it fires. While a
    // capture's subscriber is the only one alive, a call site that another test's thread fires
    // first is cached as disabled until the next subscriber is created. So the line fires once
    // here, and the captures below each create a subscriber after it is cached.
    capture_logs(|| set(&fx, "ui.color", "never"));

    let (_, logs) = capture_logs(|| set(&fx, name, "me@example.com {5h}%"));
    let info: Vec<&String> = logs.iter().filter(|l| l.contains("INFO")).collect();
    assert_eq!(info.len(), 1, "{logs:?}");
    assert!(
        info[0].contains("settings written") && info[0].contains(&key_field),
        "{info:?}"
    );
    assert!(
        logs.iter().all(|l| !l.contains("me@example.com")),
        "{logs:?}"
    );

    let (_, logs) = capture_logs(|| set(&fx, name, "me@example.com {5h}%"));
    assert!(
        logs.iter().all(|l| !l.contains("INFO")),
        "a change to nothing logs nothing: {logs:?}"
    );

    let (_, logs) = capture_logs(|| unset(&fx, name));
    assert_eq!(
        logs.iter()
            .filter(|l| l.contains("INFO") && l.contains(&key_field))
            .count(),
        1,
        "{logs:?}"
    );
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use super::*;

    /// The second writer's thread, started from inside the first writer's lock.
    type Waiter = Arc<Mutex<Option<JoinHandle<Result<bool, String>>>>>;

    #[test]
    fn a_writer_waits_while_another_holds_the_settings_lock_and_both_keys_land() {
        // §6.4: the read, the edit and the write happen under the lock. A second writer starts
        // while the first holds the lock between its read and its write. Without the lock it
        // would read the file before the first write lands, and the first write would then
        // drop its key.
        let fx = Fx::new();
        let other = Arc::new(fx.engine_with_env(fx.env.clone()));
        let waiter: Waiter = Arc::default();
        let (writer, slot) = (other.clone(), waiter.clone());
        fx.engine.on_point(
            "settings-read",
            Box::new(move || {
                let writer = writer.clone();
                *slot.lock().unwrap() = Some(thread::spawn(move || {
                    writer
                        .config_set("ui.color", None, "never")
                        .map(|change| change.changed)
                        .map_err(|e| e.to_string())
                }));
                // The second writer is now waiting for the lock this one holds.
                thread::sleep(Duration::from_millis(300));
            }),
        );

        assert!(set(&fx, "autoswitch.threshold", "85.5").changed);

        let second = waiter
            .lock()
            .unwrap()
            .take()
            .expect("the second writer started");
        assert_eq!(second.join().unwrap(), Ok(true));
        assert_eq!(
            config_text(&fx),
            "[autoswitch]\nthreshold = 85.5\n\n[ui]\ncolor = \"never\"\n"
        );
    }
}
