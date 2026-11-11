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
use tagteam_engine::config::{MappingWrite, SETTINGS_LOCK_TIMEOUT, SettingsChange};
use tagteam_engine::settings::{
    AccountsView, ColorMode, KEYS, Key, KeyKind, Settings, SettingsError, Value, config_path,
};
use tagteam_engine::{Engine, EngineError};
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
fn replacing_a_list_keeps_the_comment_of_each_element_it_keeps() {
    // §6.4: an element that stays keeps its own comments, even in a new place in the list, and
    // a new element is written in the layout the list already has.
    let fx = Fx::new();
    write_config(
        &fx,
        "\
# my models
[autoswitch]
models = [
  \"Fable\", # retain this model
]
threshold = 80.0
",
    );

    set(&fx, "autoswitch.models", "Fable,opus");
    assert_eq!(
        config_text(&fx),
        "\
# my models
[autoswitch]
models = [
  \"Fable\", # retain this model
  \"opus\",
]
threshold = 80.0
"
    );
}

#[test]
fn a_comment_before_the_closing_bracket_of_a_replaced_list_survives() {
    let fx = Fx::new();
    write_config(
        &fx,
        "\
[autoswitch]
models = [
  \"Fable\",
  # the models I use
  \"opus\", # keep opus
  # last word
]
",
    );

    set(&fx, "autoswitch.models", "Fable,opus,sonnet");
    assert_eq!(
        config_text(&fx),
        "\
[autoswitch]
models = [
  \"Fable\",
  # the models I use
  \"opus\", # keep opus
  \"sonnet\",
  # last word
]
"
    );
}

#[test]
fn the_comment_of_an_element_a_replacement_drops_goes_with_it() {
    let fx = Fx::new();
    write_config(
        &fx,
        "\
[autoswitch]
models = [
  \"Fable\", # stays
  \"opus\", # goes with opus
  \"sonnet\", # stays too
]
",
    );

    set(&fx, "autoswitch.models", "Fable,sonnet");
    assert_eq!(
        config_text(&fx),
        "\
[autoswitch]
models = [
  \"Fable\", # stays
  \"sonnet\", # stays too
]
"
    );
}

#[test]
fn a_new_element_never_takes_the_comment_of_a_removed_one() {
    // The comment above `Fable` goes with `Fable`, whichever layout the list has: a new
    // element takes the indentation and line breaks of the old list and no comment text.
    let fx = Fx::new();
    write_config(
        &fx,
        "\
[autoswitch]
models = [
  # Fable only
  \"Fable\", \"opus\"
]
",
    );
    set(&fx, "autoswitch.models", "sonnet");
    assert_eq!(
        config_text(&fx),
        "\
[autoswitch]
models = [
  \"sonnet\"
]
"
    );

    write_config(
        &fx,
        "\
[autoswitch]
models = [
  # Fable only
  \"Fable\", # and its line
  \"opus\",
]
",
    );
    set(&fx, "autoswitch.models", "sonnet,opus");
    assert_eq!(
        config_text(&fx),
        "\
[autoswitch]
models = [
  \"sonnet\",
  \"opus\",
]
"
    );
}

#[test]
fn replacing_a_one_line_list_keeps_it_on_one_line() {
    let fx = Fx::new();
    write_config(
        &fx,
        "[autoswitch]\nmodels = [\"Fable\", \"opus\"]   # mine\n",
    );

    set(&fx, "autoswitch.models", "Fable,opus,sonnet");
    assert_eq!(
        config_text(&fx),
        "[autoswitch]\nmodels = [\"Fable\", \"opus\", \"sonnet\"]   # mine\n"
    );
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
        ("tokens.retention_days", &["30", "3650"], &["29", "3651"]),
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
            "ui.accounts_view",
            &["list", "compact"],
            &["full", "List", ""],
        ),
        (
            "statusline.format",
            &["{account} {5h}%", "{model:Fable}"],
            &["", "{bogus}", "{5h"],
        ),
        ("run.share_extra", &["hook-data", ""], &["a/b", ".."]),
        ("daemon.autoswitch", &["no", "true"], &["paused", "2", ""]),
        ("notify.enabled", &["false", "yes"], &["off"]),
        (
            "notify.events",
            &["limit, switch", ""],
            &["beep", "switch,,limit", "Switch"],
        ),
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
    assert_eq!(settings.accounts_view, AccountsView::Compact);
    assert_eq!(
        (
            settings.daemon_autoswitch,
            settings.notify_enabled,
            settings.notify_events
        ),
        (true, true, Vec::new())
    );
}

#[test]
fn every_numeric_key_in_the_registry_refuses_a_step_past_either_end() {
    // §6.4: `set` never clamps. Each range comes from the registry, so a key added to it is
    // covered without a new case here.
    let fx = Fx::new();
    for key in KEYS {
        let outside = match key.kind {
            KeyKind::Float(r) => [r.start() - 0.01, r.end() + 0.01].map(|v| v.to_string()),
            KeyKind::Int(r) => [r.start() - 1, r.end() + 1].map(|v| v.to_string()),
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

/// What a `config set` that must refuse says.
fn refusal_message(
    engine: &Engine,
    name: &str,
    provider: Option<&ProviderId>,
    raw: &str,
) -> String {
    let err = engine.config_set(name, provider, raw).unwrap_err();
    assert_eq!(err.kind(), "invalid-input", "{name} {raw}");
    err.to_string()
}

/// What a `config unset` that must refuse says.
fn unset_refusal_message(engine: &Engine, name: &str) -> String {
    let err = engine.config_unset(name, None).unwrap_err();
    assert_eq!(err.kind(), "invalid-input", "{name}");
    err.to_string()
}

#[test]
fn a_write_that_breaks_the_pair_is_refused_and_nothing_is_written() {
    // S3.2: a write after which a provider's threshold is at or above its ceiling is refused, from
    // either side of the pair, and the file is left as it was.
    let fx = Fx::new();
    let file = "[autoswitch]\nthreshold = 90\n";
    write_config(&fx, file);

    assert_eq!(
        refusal_message(&fx.engine, "autoswitch.ceiling", None, "90"),
        "`autoswitch.ceiling` must be above `autoswitch.threshold` (claude-code would have a threshold of 90 and a ceiling of 90)"
    );
    assert_eq!(config_text(&fx), file);

    assert!(set(&fx, "autoswitch.ceiling", "95").changed);
    let held = config_text(&fx);
    assert_eq!(
        refusal_message(&fx.engine, "autoswitch.threshold", None, "95"),
        "`autoswitch.threshold` must be below `autoswitch.ceiling` (claude-code would have a threshold of 95 and a ceiling of 95)"
    );
    assert_eq!(config_text(&fx), held);
}

#[test]
fn both_spellings_of_a_provider_entry_are_refused_alike() {
    let fx = Fx::new();
    let file = "[autoswitch]\nthreshold = 90\n";
    write_config(&fx, file);
    let expected = "`provider.claude-code.autoswitch.ceiling` must be above `autoswitch.threshold` (claude-code would have a threshold of 90 and a ceiling of 85)";

    assert_eq!(
        refusal_message(
            &fx.engine,
            "provider.claude-code.autoswitch.ceiling",
            None,
            "85"
        ),
        expected
    );
    assert_eq!(
        refusal_message(&fx.engine, "autoswitch.ceiling", Some(&cc()), "85"),
        expected
    );
    assert_eq!(config_text(&fx), file);
}

#[test]
fn an_unset_that_breaks_the_pair_is_refused() {
    // Removing a threshold lets the global (or default) one back in, which can sit at or above
    // the ceiling that held beside the old one.
    let fx = Fx::new();
    let file = "[autoswitch]\nthreshold = 80\nceiling = 85\n";
    write_config(&fx, file);
    assert_eq!(
        unset_refusal_message(&fx.engine, "autoswitch.threshold"),
        "`autoswitch.threshold` must be below `autoswitch.ceiling` (claude-code would have a threshold of 90 and a ceiling of 85)"
    );
    assert_eq!(config_text(&fx), file);
    assert!(unset(&fx, "autoswitch.ceiling").changed);

    let file = "[autoswitch]\nthreshold = 92\n[provider.claude-code.autoswitch]\nthreshold = 80\nceiling = 85\n";
    write_config(&fx, file);
    assert_eq!(
        unset_refusal_message(&fx.engine, "provider.claude-code.autoswitch.threshold"),
        "`provider.claude-code.autoswitch.threshold` must be below `autoswitch.ceiling` (claude-code would have a threshold of 92 and a ceiling of 85)"
    );
    assert_eq!(config_text(&fx), file);
}

#[test]
fn a_global_write_is_checked_for_every_registered_provider() {
    // A global ceiling reaches every provider, so a provider whose own threshold it passes
    // refuses it even though claude-code, which comes first, would hold.
    let ffx = FakeFx::new();
    let file = "[provider.fake-agent.autoswitch]\nthreshold = 96\n";
    write_config(&ffx.fx, file);
    assert_eq!(
        refusal_message(&ffx.engine, "autoswitch.ceiling", None, "95"),
        "`autoswitch.ceiling` must be above `autoswitch.threshold` (fake-agent would have a threshold of 96 and a ceiling of 95)"
    );
    assert_eq!(config_text(&ffx.fx), file);
}

#[test]
fn a_pair_the_file_already_breaks_blocks_no_write() {
    // Review Focus 3: a file that breaks the pair already reads with a warning and a ceiling of
    // 100. A write that does not newly break it, whatever its key, is not refused.
    let fx = Fx::new();
    write_config(&fx, "[autoswitch]\nthreshold = 95\nceiling = 90\n");
    for (name, raw) in [
        ("daemon.autoswitch", "false"),
        ("autoswitch.cooldown_seconds", "600"),
        ("autoswitch.ceiling", "92"),
        ("autoswitch.ceiling", "96"),
    ] {
        assert!(set(&fx, name, raw).changed, "{name} {raw}");
    }
}

/// A value `set` takes for `key`: the bottom of a range, or the last choice. The ceiling takes
/// the top of its range: the bottom is never above a threshold, so a write refuses it.
fn sample(key: &Key) -> String {
    match &key.kind {
        KeyKind::Float(r) if key.name == "autoswitch.ceiling" => r.end().to_string(),
        KeyKind::Float(r) => r.start().to_string(),
        KeyKind::Int(r) => r.start().to_string(),
        KeyKind::Bool => "yes".into(),
        KeyKind::Choice(c) => c.values().last().unwrap().to_string(),
        KeyKind::Models => "Fable, opus".into(),
        KeyKind::Format => "{account} · {7d}%".into(),
        KeyKind::ShareNames => "hook-data".into(),
        KeyKind::Provider => cc().to_string(),
        KeyKind::NotifyEvents => "limit, switch".into(),
    }
}

#[test]
fn every_key_set_through_the_registry_reads_back_without_a_warning() {
    // §6.4 "One registry": the forgiving reader takes whatever `set` writes as it is, from the
    // global table and from a provider's.
    let fx = Fx::new();
    for key in KEYS {
        let raw = sample(key);
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
        ("ui.accounts_view", "compact"),
        ("usage.history_retention_days", "30"),
        ("tokens.retention_days", "90"),
        ("default_provider", "claude-code"),
        ("notify.enabled", "false"),
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
    let err = fx
        .engine
        .config_set("default_provider", None, "fake-agent")
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::Settings(SettingsError::UnknownProvider { id, .. }) if id == "fake-agent")
    );
    assert_eq!(
        err.to_string(),
        "unknown provider \"fake-agent\"; name one this build has, such as claude-code"
    );
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

#[test]
fn config_set_refuses_a_price_key() {
    // Token report spec §8.2: the price tables are no registry keys, so `config` writes none
    // of them; the user edits the file.
    let fx = Fx::new();
    let text = "[provider.claude-code.prices.\"claude-opus-5-5\"]\ninput = 4.0\n";
    write_config(&fx, text);
    let flag = cc();
    for (name, provider) in [
        ("provider.claude-code.prices.claude-opus-5-5.input", None),
        ("prices.claude-opus-5-5.input", Some(&flag)),
        ("provider.claude-code.prices", None),
    ] {
        assert_eq!(refusal(&fx, name, provider, "5"), "invalid-input", "{name}");
        assert_eq!(
            fx.engine.config_unset(name, provider).unwrap_err().kind(),
            "invalid-input",
            "{name}"
        );
    }
    assert_eq!(config_text(&fx), text, "the file is left as it was");
    let (settings, warnings) = Settings::load(&fx.env, &cc());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        settings.prices[&cc()]["claude-opus-5-5"].base.input,
        Some(4.0)
    );
}

/// Creates `rel` under the fixture's home and returns it canonical, as `map` is given it.
fn home_dir(fx: &Fx, rel: &str) -> std::path::PathBuf {
    let dir = fx.env.home.join(rel);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(dir).unwrap()
}

fn map(fx: &Fx, dir: &Path, value: &str) -> MappingWrite {
    fx.engine
        .write_mapping(&cc(), dir, value)
        .unwrap_or_else(|e| panic!("map {dir:?} {value:?}: {e}"))
}

fn lock_file(fx: &Fx) -> std::path::PathBuf {
    fx.env.data_dir().join("locks/config.lock")
}

#[test]
fn map_writes_a_home_relative_key_and_creates_the_table() {
    // Config-mappings spec §3.3: a directory under the home is keyed `~/…`, and the table is
    // made as `config set` makes one, in a file made 0600.
    let fx = Fx::new();
    let dir = home_dir(&fx, "Code/work");

    let written = map(&fx, &dir, "work");

    assert_eq!(
        config_text(&fx),
        "[provider.claude-code.mappings]\n\"~/Code/work\" = \"work\"\n"
    );
    assert_eq!(mode(&config_path(&fx.env)), 0o600);
    assert!(written.changed);
    assert_eq!(written.written, "~/Code/work");
    assert_eq!(written.path, config_path(&fx.env));
}

#[test]
fn map_writes_an_absolute_key_outside_the_home() {
    let fx = Fx::new();
    let outside = fx.dir.path().join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    let dir = fs::canonicalize(outside).unwrap();

    let written = map(&fx, &dir, "work");

    assert_eq!(written.written, dir.to_str().unwrap());
    assert_eq!(
        config_text(&fx),
        format!(
            "[provider.claude-code.mappings]\n\"{}\" = \"work\"\n",
            dir.display()
        )
    );
}

#[test]
fn map_writes_a_home_relative_key_when_the_home_is_a_symlink() {
    // Review focus 1: `$HOME` reached through a symlink. The directory is passed canonical, so
    // it does not start with the home as spelled; the key is still `~/…`, and it expands back to
    // the same directory.
    let fx = Fx::with(tagteam_provider::Platform::MacOs, |env| {
        let real = env.home.with_file_name("real-home");
        fs::create_dir_all(&real).unwrap();
        symlink(&real, &env.home).unwrap();
    });
    let dir = home_dir(&fx, "Code/work");
    assert!(
        !dir.starts_with(&fx.env.home),
        "{dir:?} is canonical, the home is a link"
    );

    let written = map(&fx, &dir, "work");

    assert_eq!(written.written, "~/Code/work");
    let key = tagteam_engine::settings::expand_mapping_key(&fx.env.home, &written.written);
    assert_eq!(fs::canonicalize(key.unwrap()).unwrap(), dir);
}

#[test]
fn map_rewrites_the_first_equivalent_entry_in_place_and_removes_the_others() {
    // Config-mappings spec §3.3: the first entry for the directory keeps its key and comments
    // and takes the value; every other spelling of it goes; the rest stays.
    let fx = Fx::new();
    let dir = home_dir(&fx, "w");
    write_config(
        &fx,
        &format!(
            "[provider.claude-code.mappings]\n# work\n\"~/w\" = \"a\" # old\n\"/elsewhere\" = \"x\"\n\"{}/w\" = \"b\"\n",
            fs::canonicalize(&fx.env.home).unwrap().display()
        ),
    );

    let written = map(&fx, &dir, "c");

    assert!(written.changed);
    assert_eq!(written.written, "~/w");
    assert_eq!(
        config_text(&fx),
        "[provider.claude-code.mappings]\n# work\n\"~/w\" = \"c\" # old\n\"/elsewhere\" = \"x\"\n"
    );
}

#[test]
fn map_of_the_value_already_there_writes_nothing() {
    // The no-change path of `edit`: no write, no lock file.
    let fx = Fx::new();
    let dir = home_dir(&fx, "w");
    let text = "[provider.claude-code.mappings]\n\"~/w\" = \"a\" # kept\n";
    write_config(&fx, text);
    let before = fs::metadata(config_path(&fx.env))
        .unwrap()
        .modified()
        .unwrap();

    let written = map(&fx, &dir, "a");

    assert!(!written.changed);
    assert_eq!(written.written, "~/w");
    assert_eq!(config_text(&fx), text);
    assert_eq!(
        fs::metadata(config_path(&fx.env))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
    assert!(!lock_file(&fx).exists());
}

#[test]
fn unmap_removes_equivalent_entries_and_drops_an_empty_table() {
    let fx = Fx::new();
    let dir = home_dir(&fx, "w");
    write_config(
        &fx,
        "default_provider = \"claude-code\"\n\n[provider.claude-code.mappings]\n\"~/w\" = \"a\"\n",
    );

    assert_eq!(
        fx.engine.remove_config_mappings(Some(&cc()), &dir).unwrap(),
        1
    );

    assert_eq!(config_text(&fx), "default_provider = \"claude-code\"\n");
}

#[test]
fn unmap_keeps_a_table_that_holds_a_comment() {
    let fx = Fx::new();
    let dir = home_dir(&fx, "w");
    write_config(
        &fx,
        "# my mappings\n[provider.claude-code.mappings]\n\"~/w\" = \"a\"\n",
    );

    assert_eq!(
        fx.engine.remove_config_mappings(Some(&cc()), &dir).unwrap(),
        1
    );

    assert_eq!(
        config_text(&fx),
        "# my mappings\n[provider.claude-code.mappings]\n"
    );
}

#[test]
fn unmap_without_a_provider_removes_every_provider_s_entries() {
    let fx = Fx::new();
    let dir = home_dir(&fx, "w");
    let text = "[provider.claude-code.mappings]\n\"~/w\" = \"a\"\n\n[provider.fake-agent.mappings]\n\"~/w\" = \"b\"\n\"~/other\" = \"c\"\n";
    write_config(&fx, text);

    assert_eq!(
        fx.engine.remove_config_mappings(Some(&cc()), &dir).unwrap(),
        1
    );
    assert_eq!(
        config_text(&fx),
        "[provider.fake-agent.mappings]\n\"~/w\" = \"b\"\n\"~/other\" = \"c\"\n",
        "only claude-code's entry goes"
    );

    write_config(&fx, text);
    assert_eq!(fx.engine.remove_config_mappings(None, &dir).unwrap(), 2);
    assert_eq!(
        config_text(&fx),
        "[provider.fake-agent.mappings]\n\"~/other\" = \"c\"\n"
    );
}

#[test]
fn unmap_of_a_gone_directory_matches_its_expanded_path() {
    // Config-mappings spec §3.4: a directory that is gone is compared by its expanded path.
    let fx = Fx::new();
    let gone = fx.env.home.join("gone");
    write_config(&fx, "[provider.claude-code.mappings]\n\"~/gone\" = \"a\"\n");

    assert_eq!(
        fx.engine
            .remove_config_mappings(Some(&cc()), &gone)
            .unwrap(),
        1
    );

    assert_eq!(config_text(&fx), "");
}

#[test]
fn unmap_that_matches_nothing_changes_nothing() {
    let fx = Fx::new();
    let dir = home_dir(&fx, "w");
    let text = "[provider.claude-code.mappings]\n\"~/other\" = \"a\"\n";
    write_config(&fx, text);

    assert_eq!(
        fx.engine.remove_config_mappings(Some(&cc()), &dir).unwrap(),
        0
    );

    assert_eq!(config_text(&fx), text);
    assert!(!lock_file(&fx).exists());
}

#[test]
fn map_and_unmap_keep_an_inline_mappings_table_inline() {
    // Review focus 4: a hand-written inline table is edited in its own style.
    let fx = Fx::new();
    let x = home_dir(&fx, "x");
    let y = home_dir(&fx, "y");
    write_config(
        &fx,
        "[provider.claude-code]\nmappings = { \"~/x\" = \"a\" }\n",
    );

    map(&fx, &y, "b");
    assert_eq!(
        config_text(&fx),
        "[provider.claude-code]\nmappings = { \"~/x\" = \"a\", \"~/y\" = \"b\" }\n"
    );

    assert_eq!(
        fx.engine.remove_config_mappings(Some(&cc()), &x).unwrap(),
        1
    );
    assert_eq!(
        fx.engine.remove_config_mappings(Some(&cc()), &y).unwrap(),
        1
    );
    // As `config unset` does, the tables the removal empties go with it, the header the file
    // wrote by hand included.
    assert_eq!(config_text(&fx), "");
}

#[test]
fn a_mappings_entry_that_is_not_a_table_is_invalid() {
    let fx = Fx::new();
    let dir = home_dir(&fx, "w");
    let text = "[provider.claude-code]\nmappings = \"oops\"\n";
    write_config(&fx, text);

    for err in [
        fx.engine.write_mapping(&cc(), &dir, "a").unwrap_err(),
        fx.engine
            .remove_config_mappings(Some(&cc()), &dir)
            .unwrap_err(),
    ] {
        match err {
            EngineError::Settings(SettingsError::Invalid { key, .. }) => {
                assert_eq!(key, "provider.claude-code.mappings");
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(config_text(&fx), text);
}

#[test]
fn a_corrupt_file_is_never_written() {
    // Review focus 5: `map` refuses to write a file that is not valid TOML.
    let fx = Fx::new();
    let dir = home_dir(&fx, "w");
    let text = "[provider.claude-code.mappings\n\"~/w\" = \n";
    write_config(&fx, text);

    for err in [
        fx.engine.write_mapping(&cc(), &dir, "a").unwrap_err(),
        fx.engine
            .remove_config_mappings(Some(&cc()), &dir)
            .unwrap_err(),
    ] {
        assert!(
            matches!(err, EngineError::Settings(SettingsError::Corrupt { .. })),
            "{err:?}"
        );
    }
    assert_eq!(config_text(&fx), text);
}

#[test]
fn with_no_file_unmap_creates_nothing() {
    let fx = Fx::new();
    let dir = home_dir(&fx, "w");

    assert_eq!(fx.engine.remove_config_mappings(None, &dir).unwrap(), 0);

    assert!(!config_path(&fx.env).exists());
    assert!(!lock_file(&fx).exists());
}

#[test]
fn a_mapping_write_logs_its_key_and_never_the_directory_or_value() {
    // Core §14.2: the log records `provider.<id>.mappings` and an action, nothing more.
    let fx = Fx::new();
    let dir = home_dir(&fx, "secret-project");
    let private = |logs: &[String]| {
        logs.iter()
            .all(|l| !l.contains("secret-project") && !l.contains("me@example.com"))
    };

    let (_, logs) = capture_logs(|| map(&fx, &dir, "me@example.com"));
    let info: Vec<&String> = logs.iter().filter(|l| l.contains("INFO")).collect();
    assert_eq!(info.len(), 1, "{logs:?}");
    assert!(
        info[0].contains("settings written")
            && info[0].contains("provider.claude-code.mappings")
            && info[0].contains("action=\"map\""),
        "{info:?}"
    );
    assert!(private(&logs), "{logs:?}");

    let (_, logs) = capture_logs(|| fx.engine.remove_config_mappings(None, &dir).unwrap());
    let info: Vec<&String> = logs.iter().filter(|l| l.contains("INFO")).collect();
    assert_eq!(info.len(), 1, "{logs:?}");
    assert!(
        info[0].contains("settings written")
            && info[0].contains("provider.claude-code.mappings")
            && info[0].contains("action=\"unmap\""),
        "{info:?}"
    );
    assert!(private(&logs), "{logs:?}");
}

#[test]
fn unmapping_the_last_entry_of_an_inline_table_keeps_the_space_before_the_brace() {
    let fx = Fx::new();
    let y = home_dir(&fx, "y");
    let text = "[provider.claude-code]\nmappings = { \"~/x\" = \"a\", \"~/y\" = \"b\" }\n";
    write_config(&fx, text);

    assert_eq!(
        fx.engine.remove_config_mappings(Some(&cc()), &y).unwrap(),
        1
    );

    assert_eq!(
        config_text(&fx),
        "[provider.claude-code]\nmappings = { \"~/x\" = \"a\" }\n"
    );
}

#[test]
fn map_removes_a_later_equivalent_entry_even_when_the_first_holds_the_value() {
    // The value is already there, but a second spelling of the directory is not: the write
    // drops it and reports the change, under the first entry's key.
    let fx = Fx::new();
    let dir = home_dir(&fx, "w");
    write_config(
        &fx,
        &format!(
            "[provider.claude-code.mappings]\n\"~/w\" = \"a\"\n\"{}/w\" = \"b\"\n",
            fs::canonicalize(&fx.env.home).unwrap().display()
        ),
    );

    let written = map(&fx, &dir, "a");

    assert!(written.changed);
    assert_eq!(written.written, "~/w");
    assert_eq!(
        config_text(&fx),
        "[provider.claude-code.mappings]\n\"~/w\" = \"a\"\n"
    );
}
