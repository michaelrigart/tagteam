use std::fs;
use std::time::{Duration, SystemTime};

use serde_json::json;
use tagteam_core::ProviderId;
use tagteam_core::autoswitch::Strategy;
use tagteam_engine::settings::{
    self, COOLDOWN_SECONDS_RANGE, ColorMode, HYSTERESIS_PCT_RANGE, INTERVAL_SECONDS_RANGE,
    Inspection, KEYS, Key, KeyKind, KeyState, STATUSLINE_PLACEHOLDERS, Settings, SettingsError,
    Source, THRESHOLD_RANGE, UNHEALTHY_TICKS_RANGE, Value, inspect, is_statusline_placeholder,
    parse_bool, resolve,
};
use tagteam_provider::Env;

const PROVIDER: &str = "claude-code";

/// Loads `text` as the `config.toml` of a fresh environment.
fn load_as(text: &str, provider: &str) -> (Settings, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.config_dir()).unwrap();
    fs::write(env.config_dir().join("config.toml"), text).unwrap();
    Settings::load(&env, &ProviderId::new(provider))
}

fn load(text: &str) -> (Settings, Vec<String>) {
    load_as(text, PROVIDER)
}

fn models(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_defaults_are_the_specs_table() {
    let d = Settings::default();
    assert_eq!(d.default_provider, ProviderId::new("claude-code"));
    assert_eq!(d.threshold, 90.0);
    assert_eq!(d.interval_seconds, 60);
    assert_eq!(d.cooldown_seconds, 300);
    assert_eq!(d.hysteresis_pct, 10.0);
    assert_eq!(d.strategy, Strategy::Best);
    assert!(!d.include_api_key_accounts);
    assert_eq!(d.unhealthy_ticks, 3);
    assert_eq!(d.models, Vec::<String>::new());
    assert_eq!(d.history_retention_days, 180);
    assert_eq!(
        d.statusline_format,
        "{account} · 5h {5h}% · 7d {7d}%{stale}"
    );
    assert_eq!(d.color, ColorMode::Auto);
    assert!(d.share_extra.is_empty());
}

#[test]
fn a_missing_file_gives_the_defaults_without_a_warning() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    let (settings, warnings) = Settings::load(&env, &ProviderId::new(PROVIDER));
    assert_eq!(settings, Settings::default());
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(
        !env.config_dir().exists(),
        "loading never creates the config directory"
    );
}

#[test]
fn an_empty_file_gives_the_defaults_without_a_warning() {
    let (settings, warnings) = load("");
    assert_eq!(settings, Settings::default());
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn every_key_is_read_from_a_full_file() {
    let (settings, warnings) = load(
        r#"
default_provider = "fake-agent"

[autoswitch]
threshold = 75.5
interval_seconds = 120
cooldown_seconds = 0
hysteresis_pct = 12.5
strategy = "consume-first"
include_api_key_accounts = true
unhealthy_ticks = 5
models = ["Fable", "Opus"]

[usage]
history_retention_days = 30

[statusline]
format = "{5h}%"

[ui]
color = "never"

[run]
share_extra = ["hook-data"]
"#,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        settings,
        Settings {
            default_provider: ProviderId::new("fake-agent"),
            threshold: 75.5,
            interval_seconds: 120,
            cooldown_seconds: 0,
            hysteresis_pct: 12.5,
            strategy: Strategy::ConsumeFirst,
            include_api_key_accounts: true,
            unhealthy_ticks: 5,
            models: models(&["Fable", "Opus"]),
            history_retention_days: 30,
            statusline_format: "{5h}%".to_owned(),
            color: ColorMode::Never,
            share_extra: vec!["hook-data".into()],
        }
    );
}

#[test]
fn keys_written_with_dotted_names_are_read_too() {
    let (settings, warnings) = load("autoswitch.threshold = 80\nui.color = \"always\"\n");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(settings.threshold, 80.0);
    assert_eq!(settings.color, ColorMode::Always);
}

#[test]
fn an_unknown_key_is_ignored_without_a_warning() {
    let (settings, warnings) =
        load("colour = \"never\"\n[autoswitch]\nfuture = true\n[elsewhere]\nx = 1\n");
    assert_eq!(settings, Settings::default());
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn the_threshold_accepts_its_whole_range_as_an_integer_or_a_float() {
    for (text, expected) in [
        ("50", 50.0),
        ("50.0", 50.0),
        ("75", 75.0),
        ("99.9", 99.9),
        ("90.25", 90.25),
    ] {
        let (settings, warnings) = load(&format!("[autoswitch]\nthreshold = {text}\n"));
        assert_eq!(settings.threshold, expected, "{text}");
        assert!(warnings.is_empty(), "{text}: {warnings:?}");
    }
}

#[test]
fn an_invalid_threshold_falls_back_to_ninety_with_one_warning_naming_the_key() {
    for text in [
        "49.9", "49", "99.95", "100", "0", "-90", "\"90\"", "true", "nan", "inf", "[90]",
    ] {
        let (settings, warnings) = load(&format!("[autoswitch]\nthreshold = {text}\n"));
        assert_eq!(settings.threshold, 90.0, "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(
            warnings[0].contains("`autoswitch.threshold`"),
            "{warnings:?}"
        );
    }
}

#[test]
fn the_history_retention_accepts_one_to_thirty_six_fifty() {
    for days in [1u32, 180, 3650] {
        let (settings, warnings) = load(&format!("[usage]\nhistory_retention_days = {days}\n"));
        assert_eq!(settings.history_retention_days, days);
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}

#[test]
fn an_invalid_history_retention_falls_back_to_one_eighty_with_a_warning() {
    for text in ["0", "3651", "-5", "1.5", "\"30\"", "true", "4294967297"] {
        let (settings, warnings) = load(&format!("[usage]\nhistory_retention_days = {text}\n"));
        assert_eq!(settings.history_retention_days, 180, "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(
            warnings[0].contains("`usage.history_retention_days`"),
            "{warnings:?}"
        );
    }
}

#[test]
fn models_take_a_list_or_a_single_name() {
    let (settings, warnings) = load("[autoswitch]\nmodels = \"Fable\"\n");
    assert_eq!(settings.models, models(&["Fable"]));
    assert!(warnings.is_empty(), "{warnings:?}");

    let (settings, warnings) = load("[autoswitch]\nmodels = [\"Fable\", \" Opus \"]\n");
    assert_eq!(
        settings.models,
        models(&["Fable", "Opus"]),
        "names are trimmed"
    );
    assert!(warnings.is_empty(), "trimming is silent: {warnings:?}");

    let (settings, _) = load("[autoswitch]\nmodels = [\"all\"]\n");
    assert_eq!(settings.models, models(&["all"]));

    let (settings, warnings) = load("[autoswitch]\nmodels = []\n");
    assert_eq!(settings.models, Vec::<String>::new());
    assert!(warnings.is_empty(), "an empty list is valid: {warnings:?}");
}

#[test]
fn an_invalid_models_value_falls_back_to_no_models_with_a_warning() {
    for text in [
        "[\"Fable\", 3]",
        "[[\"Fable\"]]",
        "[\"\"]",
        "\"\"",
        "5",
        "true",
    ] {
        let (settings, warnings) = load(&format!("[autoswitch]\nmodels = {text}\n"));
        assert_eq!(settings.models, Vec::<String>::new(), "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(warnings[0].contains("`autoswitch.models`"), "{warnings:?}");
    }
}

#[test]
fn a_providers_own_models_override_the_global_ones() {
    let (settings, warnings) = load(
        "[autoswitch]\nmodels = [\"Opus\"]\n\n[provider.claude-code.autoswitch]\nmodels = [\"Fable\"]\n",
    );
    assert_eq!(settings.models, models(&["Fable"]));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn another_providers_override_does_not_apply() {
    let text = "[autoswitch]\nmodels = [\"Opus\"]\n\n[provider.fake-agent.autoswitch]\nmodels = [\"Fable\"]\n";
    assert_eq!(load(text).0.models, models(&["Opus"]));
    assert_eq!(load_as(text, "fake-agent").0.models, models(&["Fable"]));
}

#[test]
fn a_providers_override_without_the_key_leaves_the_global_value() {
    let (settings, warnings) = load(
        "[autoswitch]\nmodels = [\"Opus\"]\nthreshold = 80\n\n[provider.claude-code.autoswitch]\nmodels = [\"Fable\"]\n",
    );
    assert_eq!(settings.models, models(&["Fable"]));
    assert_eq!(settings.threshold, 80.0);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_providers_own_threshold_overrides_the_global_one() {
    let text =
        "[autoswitch]\nthreshold = 80\n\n[provider.claude-code.autoswitch]\nthreshold = 70\n";
    let (settings, warnings) = load(text);
    assert_eq!(settings.threshold, 70.0);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(load_as(text, "fake-agent").0.threshold, 80.0);
}

#[test]
fn an_invalid_provider_threshold_warns_and_the_global_value_applies() {
    let (settings, warnings) = load(
        "[autoswitch]\nthreshold = 80\n\n[provider.claude-code.autoswitch]\nthreshold = 120\n",
    );
    assert_eq!(settings.threshold, 80.0);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("`provider.claude-code.autoswitch.threshold`"),
        "{warnings:?}"
    );
}

#[test]
fn an_invalid_provider_override_warns_and_the_global_value_applies() {
    let (settings, warnings) = load(
        "[autoswitch]\nmodels = [\"Opus\"]\n\n[provider.claude-code.autoswitch]\nmodels = 7\n",
    );
    assert_eq!(settings.models, models(&["Opus"]));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("`provider.claude-code.autoswitch.models`"),
        "{warnings:?}"
    );
}

#[test]
fn the_statusline_format_prefers_the_providers_table() {
    let (settings, warnings) = load(
        "[statusline]\nformat = \"{5h}\"\n\n[provider.claude-code.statusline]\nformat = \"{7d}\"\n",
    );
    assert_eq!(settings.statusline_format, "{7d}");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        load("[statusline]\nformat = \"{5h}\"\n")
            .0
            .statusline_format,
        "{5h}"
    );
}

#[test]
fn an_empty_or_blank_statusline_format_falls_back_with_a_warning() {
    for text in ["\"\"", "\"   \"", "5", "[\"x\"]"] {
        let (settings, warnings) = load(&format!("[statusline]\nformat = {text}\n"));
        assert_eq!(
            settings.statusline_format, "{account} · 5h {5h}% · 7d {7d}%{stale}",
            "{text}"
        );
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(warnings[0].contains("`statusline.format`"), "{warnings:?}");
    }
}

#[test]
fn the_color_mode_reads_its_three_values() {
    for (text, mode) in [
        ("auto", ColorMode::Auto),
        ("always", ColorMode::Always),
        ("never", ColorMode::Never),
    ] {
        let (settings, warnings) = load(&format!("[ui]\ncolor = \"{text}\"\n"));
        assert_eq!(settings.color, mode);
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}

#[test]
fn an_invalid_color_falls_back_to_auto_with_a_warning() {
    for text in ["\"rainbow\"", "\"Always\"", "true", "1"] {
        let (settings, warnings) = load(&format!("[ui]\ncolor = {text}\n"));
        assert_eq!(settings.color, ColorMode::Auto, "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(warnings[0].contains("`ui.color`"), "{warnings:?}");
    }
}

#[test]
fn each_invalid_value_warns_once_and_the_valid_ones_still_apply() {
    let (settings, warnings) = load(
        r#"
[autoswitch]
threshold = 120
models = ["Fable"]

[usage]
history_retention_days = 0

[ui]
color = "always"
"#,
    );
    assert_eq!(settings.threshold, 90.0);
    assert_eq!(settings.models, models(&["Fable"]));
    assert_eq!(settings.history_retention_days, 180);
    assert_eq!(settings.color, ColorMode::Always);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("`autoswitch.threshold`"))
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("`usage.history_retention_days`"))
    );
}

#[test]
fn a_table_that_is_not_a_table_warns_once_and_reads_as_absent() {
    let (settings, warnings) = load("autoswitch = 5\n");
    assert_eq!(settings, Settings::default());
    assert_eq!(
        warnings.len(),
        1,
        "threshold and models share one warning: {warnings:?}"
    );
    assert!(
        warnings[0].contains("`autoswitch` must be a table"),
        "{warnings:?}"
    );
}

#[test]
fn a_corrupt_file_gives_the_defaults_and_one_warning_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.config_dir()).unwrap();
    let path = env.config_dir().join("config.toml");
    fs::write(&path, "[autoswitch\nthreshold = = 3\n").unwrap();
    let (settings, warnings) = Settings::load(&env, &ProviderId::new(PROVIDER));
    assert_eq!(settings, Settings::default());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains(&path.display().to_string()),
        "{warnings:?}"
    );
    assert!(warnings[0].contains("not valid TOML"), "{warnings:?}");
}

#[test]
fn an_unreadable_file_gives_the_defaults_and_one_warning_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    // A directory where the file should be: reading it fails, and it is not "missing".
    let path = env.config_dir().join("config.toml");
    fs::create_dir_all(&path).unwrap();
    let (settings, warnings) = Settings::load(&env, &ProviderId::new(PROVIDER));
    assert_eq!(settings, Settings::default());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains(&path.display().to_string()),
        "{warnings:?}"
    );
    assert!(warnings[0].contains("cannot read"), "{warnings:?}");
}

#[test]
fn a_file_that_is_not_utf8_reads_as_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.config_dir()).unwrap();
    let path = env.config_dir().join("config.toml");
    fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
    let (settings, warnings) = Settings::load(&env, &ProviderId::new(PROVIDER));
    assert_eq!(settings, Settings::default());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains(&path.display().to_string()),
        "{warnings:?}"
    );
    assert!(warnings[0].contains("cannot read"), "{warnings:?}");
}

#[test]
fn a_comment_heavy_file_with_inline_tables_reads_normally() {
    let (settings, warnings) =
        load("# my settings\nautoswitch = { threshold = 60, models = [\"all\"] } # inline\n");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(settings.threshold, 60.0);
    assert_eq!(settings.models, models(&["all"]));
}

#[test]
fn a_provider_key_that_is_not_a_table_warns_once_across_models_and_format() {
    let (settings, warnings) = load("provider = 5\n");
    assert_eq!(settings, Settings::default());
    assert_eq!(
        warnings.len(),
        1,
        "threshold, models and format share one warning: {warnings:?}"
    );
    assert!(
        warnings[0].contains("`provider` must be a table"),
        "{warnings:?}"
    );
}

#[test]
fn an_array_of_tables_is_not_a_table_and_gives_the_default_with_a_warning() {
    let (settings, warnings) = load("[[autoswitch]]\nthreshold = 60\nmodels = [\"Fable\"]\n");
    assert_eq!(settings, Settings::default());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("`autoswitch` must be a table"),
        "{warnings:?}"
    );
}

#[test]
fn every_settings_warning_names_the_full_path_of_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.config_dir()).unwrap();
    let path = env.config_dir().join("config.toml");
    fs::write(
        &path,
        "autoswitch = 5\n[usage]\nhistory_retention_days = 0\n[ui]\ncolor = \"rainbow\"\n[statusline]\nformat = \"{nope}\"\n",
    )
    .unwrap();
    let (_, warnings) = Settings::load(&env, &ProviderId::new(PROVIDER));
    assert_eq!(warnings.len(), 4, "{warnings:?}");
    let prefix = format!("{}: ", path.display());
    for warning in &warnings {
        assert!(warning.starts_with(&prefix), "{warning}");
    }
}

#[test]
fn the_placeholder_list_is_the_specs_section_thirteen_five() {
    assert_eq!(
        STATUSLINE_PLACEHOLDERS,
        [
            "account", "position", "email", "5h", "7d", "5h_reset", "7d_reset", "spend", "stale"
        ]
    );
    for name in STATUSLINE_PLACEHOLDERS {
        assert!(is_statusline_placeholder(name), "{name}");
    }
    assert!(is_statusline_placeholder("model:Fable"));
    assert!(is_statusline_placeholder("model:Fable Pro"));
    for name in [
        "",
        "nope",
        "model",
        "model:",
        "model: ",
        "model: Fable",
        "model:Fable ",
        "model:{5h",
        "model:5h}",
        "model:{x}",
        "Account",
        "5H",
        " 5h",
        "5h ",
        "{5h}",
        "5h}",
    ] {
        assert!(!is_statusline_placeholder(name), "{name:?}");
    }
}

#[test]
fn a_format_made_of_known_placeholders_and_plain_text_is_accepted_as_written() {
    for format in [
        "{account} · 5h {5h}% · 7d {7d}%{stale}",
        "{position}:{email} {5h_reset}/{7d_reset} {spend} {model:Fable}",
        "plain text, no placeholders",
        "} stray close brace {5h}",
        "{5h}{7d}",
    ] {
        let text = format!("[statusline]\nformat = {format:?}\n");
        let (settings, warnings) = load(&text);
        assert_eq!(settings.statusline_format, format);
        assert!(warnings.is_empty(), "{format}: {warnings:?}");
    }
}

#[test]
fn a_format_with_an_unknown_or_unclosed_placeholder_falls_back_with_one_warning() {
    for format in [
        "{nope}",
        "{account} {nope} {5h}",
        "{}",
        "{model}",
        "{model:}",
        "{model: }",
        "{model: Fable}",
        "{model:Fable }",
        "{model:{5h}",
        "{5H}",
        "{ 5h }",
        "{5h",
        "{account} {5h",
        "{{5h}}",
        "{",
    ] {
        let text = format!("[statusline]\nformat = {format:?}\n");
        let (settings, warnings) = load(&text);
        assert_eq!(
            settings.statusline_format, "{account} · 5h {5h}% · 7d {7d}%{stale}",
            "{format}"
        );
        assert_eq!(warnings.len(), 1, "{format}: {warnings:?}");
        assert!(warnings[0].contains("`statusline.format`"), "{warnings:?}");
    }
}

#[test]
fn an_invalid_provider_format_warns_and_the_global_one_applies() {
    let (settings, warnings) = load(
        "[statusline]\nformat = \"{5h}\"\n\n[provider.claude-code.statusline]\nformat = \"{nope}\"\n",
    );
    assert_eq!(settings.statusline_format, "{5h}");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("`provider.claude-code.statusline.format`"),
        "{warnings:?}"
    );
}

#[test]
fn the_format_warning_lists_the_valid_placeholders() {
    let (_, warnings) = load("[statusline]\nformat = \"{nope}\"\n");
    for name in STATUSLINE_PLACEHOLDERS {
        assert!(warnings[0].contains(&format!("{{{name}}}")), "{warnings:?}");
    }
    assert!(warnings[0].contains("{model:<name>}"), "{warnings:?}");
}

#[test]
fn all_mixed_with_model_names_is_invalid_in_any_case_and_any_position() {
    for list in [
        "[\"all\", \"Fable\"]",
        "[\"Fable\", \"all\"]",
        "[\"ALL\", \"Fable\"]",
        "[\"Fable\", \" All \"]",
    ] {
        let (settings, warnings) = load(&format!("[autoswitch]\nmodels = {list}\n"));
        assert_eq!(settings.models, Vec::<String>::new(), "{list}");
        assert_eq!(warnings.len(), 1, "{list}: {warnings:?}");
        assert!(warnings[0].contains("`autoswitch.models`"), "{warnings:?}");
    }
}

#[test]
fn a_mixed_provider_list_warns_and_the_global_models_apply() {
    let (settings, warnings) = load(
        "[autoswitch]\nmodels = [\"Opus\"]\n\n[provider.claude-code.autoswitch]\nmodels = [\"all\", \"Fable\"]\n",
    );
    assert_eq!(settings.models, models(&["Opus"]));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("`provider.claude-code.autoswitch.models`"),
        "{warnings:?}"
    );
}

#[test]
fn duplicate_model_names_collapse_silently_keeping_the_first_spelling() {
    for (list, expected) in [
        ("[\"Fable\", \"fable\"]", &["Fable"][..]),
        ("[\"fable\", \"Fable\", \"FABLE\"]", &["fable"]),
        ("[\"Fable\", \"Opus\", \" fable \"]", &["Fable", "Opus"]),
        ("[\"Opus\", \"Fable\", \"opus\"]", &["Opus", "Fable"]),
        ("[\"all\", \"ALL\"]", &["all"]),
    ] {
        let (settings, warnings) = load(&format!("[autoswitch]\nmodels = {list}\n"));
        assert_eq!(settings.models, models(expected), "{list}");
        assert!(warnings.is_empty(), "{list}: {warnings:?}");
    }
}

#[test]
fn the_autoswitch_ranges_are_the_specs_table() {
    // §6.4. A file value outside its range falls back to the default (with a warning); a CLI
    // flag is clamped into the same range.
    assert_eq!(THRESHOLD_RANGE, 50.0..=99.9);
    assert_eq!(INTERVAL_SECONDS_RANGE, 15..=3600);
    assert_eq!(COOLDOWN_SECONDS_RANGE, 0..=86_400);
    assert_eq!(HYSTERESIS_PCT_RANGE, 0.0..=50.0);
    assert_eq!(UNHEALTHY_TICKS_RANGE, 1..=100);
}

#[test]
fn the_autoswitch_numbers_accept_the_ends_of_their_ranges() {
    for (text, seconds) in [("15", 15), ("3600", 3600), ("120", 120)] {
        let (s, w) = load(&format!("[autoswitch]\ninterval_seconds = {text}\n"));
        assert_eq!((s.interval_seconds, w.len()), (seconds, 0), "{text}: {w:?}");
    }
    for (text, seconds) in [("0", 0), ("86400", 86_400)] {
        let (s, w) = load(&format!("[autoswitch]\ncooldown_seconds = {text}\n"));
        assert_eq!((s.cooldown_seconds, w.len()), (seconds, 0), "{text}: {w:?}");
    }
    for (text, pct) in [("0", 0.0), ("0.0", 0.0), ("50", 50.0), ("12.5", 12.5)] {
        let (s, w) = load(&format!("[autoswitch]\nhysteresis_pct = {text}\n"));
        assert_eq!((s.hysteresis_pct, w.len()), (pct, 0), "{text}: {w:?}");
    }
    for (text, ticks) in [("1", 1), ("100", 100)] {
        let (s, w) = load(&format!("[autoswitch]\nunhealthy_ticks = {text}\n"));
        assert_eq!((s.unhealthy_ticks, w.len()), (ticks, 0), "{text}: {w:?}");
    }
}

#[test]
fn an_invalid_autoswitch_value_falls_back_to_its_default_with_one_warning_naming_the_key() {
    // §6.4: reads are forgiving. Whole seconds and ticks are integers only, as
    // `usage.history_retention_days` is; the percentage takes an integer or a float, as
    // `autoswitch.threshold` does.
    let cases: &[(&str, &[&str])] = &[
        (
            "interval_seconds",
            &["14", "3601", "0", "-60", "60.0", "\"60\"", "true", "nan"],
        ),
        (
            "cooldown_seconds",
            &["-1", "86401", "300.5", "\"300\"", "false"],
        ),
        (
            "hysteresis_pct",
            &["-0.1", "50.1", "100", "nan", "inf", "\"10\"", "true"],
        ),
        (
            "strategy",
            &[
                "\"Best\"",
                "\"consume_first\"",
                "\"consumefirst\"",
                "\"\"",
                "1",
                "true",
                "[\"best\"]",
            ],
        ),
        (
            "include_api_key_accounts",
            &[
                "\"Yes\"", "\"TRUE\"", "\"on\"", "\"y\"", "2", "-1", "1.0", "\"\"",
            ],
        ),
        (
            "unhealthy_ticks",
            &["0", "101", "-3", "3.0", "\"3\"", "4294967299"],
        ),
    ];
    for (key, texts) in cases {
        for text in *texts {
            let (settings, warnings) = load(&format!("[autoswitch]\n{key} = {text}\n"));
            assert_eq!(settings, Settings::default(), "{key} = {text}");
            assert_eq!(warnings.len(), 1, "{key} = {text}: {warnings:?}");
            assert!(
                warnings[0].contains(&format!("`autoswitch.{key}`")),
                "{warnings:?}"
            );
        }
    }
}

#[test]
fn each_autoswitch_warning_says_what_its_key_accepts() {
    for (line, expected) in [
        (
            "interval_seconds = 5",
            "`autoswitch.interval_seconds` must be a whole number of seconds from 15 to 3600 (ignored)",
        ),
        (
            "cooldown_seconds = -5",
            "`autoswitch.cooldown_seconds` must be a whole number of seconds from 0 to 86400 (ignored)",
        ),
        (
            "hysteresis_pct = 60",
            "`autoswitch.hysteresis_pct` must be a number from 0 to 50 (ignored)",
        ),
        (
            "strategy = \"worst\"",
            "`autoswitch.strategy` must be \"best\" or \"consume-first\" (ignored)",
        ),
        (
            "include_api_key_accounts = \"maybe\"",
            "`autoswitch.include_api_key_accounts` must be true, false, 1, 0, yes or no (ignored)",
        ),
        (
            "unhealthy_ticks = 0",
            "`autoswitch.unhealthy_ticks` must be a whole number of ticks from 1 to 100 (ignored)",
        ),
    ] {
        let (_, warnings) = load(&format!("[autoswitch]\n{line}\n"));
        assert_eq!(warnings.len(), 1, "{line}: {warnings:?}");
        assert!(warnings[0].ends_with(expected), "{line}: {warnings:?}");
    }
}

#[test]
fn a_boolean_reads_only_true_false_one_zero_yes_and_no() {
    // §6.4. In the file: a TOML boolean, the integers 1 and 0, or one of the six words as a
    // string, in lower case. `parse_bool` is the words alone, for a flag's value.
    for (text, expected) in [
        ("true", true),
        ("false", false),
        ("1", true),
        ("0", false),
        ("\"true\"", true),
        ("\"false\"", false),
        ("\"1\"", true),
        ("\"0\"", false),
        ("\"yes\"", true),
        ("\"no\"", false),
    ] {
        let (settings, warnings) = load(&format!(
            "[autoswitch]\ninclude_api_key_accounts = {text}\n"
        ));
        assert_eq!(settings.include_api_key_accounts, expected, "{text}");
        assert!(warnings.is_empty(), "{text}: {warnings:?}");
    }
    for (word, expected) in [
        ("true", Some(true)),
        ("false", Some(false)),
        ("1", Some(true)),
        ("0", Some(false)),
        ("yes", Some(true)),
        ("no", Some(false)),
        ("Yes", None),
        ("TRUE", None),
        ("on", None),
        ("off", None),
        ("y", None),
        ("", None),
        (" yes", None),
    ] {
        assert_eq!(parse_bool(word), expected, "{word:?}");
    }
}

#[test]
fn the_strategy_reads_best_and_consume_first_by_their_names() {
    assert_eq!(Strategy::Best.as_str(), "best");
    assert_eq!(Strategy::ConsumeFirst.as_str(), "consume-first");
    for strategy in [Strategy::Best, Strategy::ConsumeFirst] {
        assert_eq!(Strategy::parse(strategy.as_str()), Some(strategy));
        let (settings, warnings) = load(&format!(
            "[autoswitch]\nstrategy = \"{}\"\n",
            strategy.as_str()
        ));
        assert_eq!(settings.strategy, strategy);
        assert!(warnings.is_empty(), "{warnings:?}");
    }
    for name in ["Best", "consume_first", "", " best"] {
        assert_eq!(Strategy::parse(name), None, "{name:?}");
    }
}

#[test]
fn a_providers_autoswitch_table_comes_first_for_every_key() {
    let global = "[autoswitch]\ninterval_seconds = 120\ncooldown_seconds = 600\nhysteresis_pct = 5\nstrategy = \"consume-first\"\ninclude_api_key_accounts = true\nunhealthy_ticks = 5\n";
    let (settings, warnings) = load(&format!(
        "{global}\n[provider.claude-code.autoswitch]\ninterval_seconds = 30\ncooldown_seconds = 0\nhysteresis_pct = 20\nstrategy = \"best\"\ninclude_api_key_accounts = false\nunhealthy_ticks = 1\n"
    ));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        (
            settings.interval_seconds,
            settings.cooldown_seconds,
            settings.hysteresis_pct,
            settings.strategy,
            settings.include_api_key_accounts,
            settings.unhealthy_ticks
        ),
        (30, 0, 20.0, Strategy::Best, false, 1)
    );

    // An invalid override warns, naming the provider's key, and the global value applies.
    let text = format!(
        "{global}\n[provider.claude-code.autoswitch]\ninterval_seconds = 1\nstrategy = \"worst\"\n"
    );
    let (settings, warnings) = load(&text);
    assert_eq!(
        (settings.interval_seconds, settings.strategy),
        (120, Strategy::ConsumeFirst)
    );
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    for key in ["interval_seconds", "strategy"] {
        let named = format!("`provider.claude-code.autoswitch.{key}`");
        assert!(warnings.iter().any(|w| w.contains(&named)), "{warnings:?}");
    }
    // Another provider reads only the global table.
    let (other, warnings) = load_as(&text, "fake-agent");
    assert_eq!(
        (other.interval_seconds, other.strategy),
        (120, Strategy::ConsumeFirst)
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn the_mtime_is_the_settings_files_and_none_without_one() {
    // §11.4: a running `auto` re-reads the file whenever this changes.
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    assert_eq!(Settings::mtime(&env), None, "no file");
    fs::create_dir_all(env.config_dir()).unwrap();
    let path = env.config_dir().join("config.toml");
    fs::write(&path, "[autoswitch]\nthreshold = 80\n").unwrap();
    let first = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
    let touch = |at: SystemTime| {
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(at)
            .unwrap()
    };
    touch(first);
    assert_eq!(Settings::mtime(&env), Some(first));
    touch(first + Duration::from_secs(1));
    assert_eq!(
        Settings::mtime(&env),
        Some(first + Duration::from_secs(1)),
        "a write moves it"
    );
    fs::remove_file(&path).unwrap();
    assert_eq!(Settings::mtime(&env), None, "a removed file has none");
}

#[test]
fn run_share_extra_is_read_from_the_provider_s_table_first() {
    let text = "[run]\nshare_extra = [\"global\"]\n\
                [provider.claude-code.run]\nshare_extra = [\"hook-data\", \"tool-cache\"]\n";
    let (settings, warnings) = load(text);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(settings.share_extra, ["hook-data", "tool-cache"]);
    let (settings, _) = load_as(text, "fake-agent");
    assert_eq!(
        settings.share_extra,
        ["global"],
        "another provider's table does not apply"
    );
}

#[test]
fn run_share_extra_takes_a_name_or_a_list_and_collapses_repeats() {
    let (settings, warnings) = load("[run]\nshare_extra = \"hook-data\"\n");
    assert_eq!(settings.share_extra, ["hook-data"]);
    assert!(warnings.is_empty(), "{warnings:?}");

    let (settings, warnings) =
        load("[run]\nshare_extra = [\"hooks.json\", \".my-tool\", \"hooks.json\"]\n");
    assert_eq!(
        settings.share_extra,
        ["hooks.json", ".my-tool"],
        "a dot inside a name is fine"
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_share_extra_item_that_is_not_an_entry_name_is_dropped_with_a_warning_naming_it() {
    let (settings, warnings) =
        load("[run]\nshare_extra = [\"hook-data\", \"\", \".\", \"..\", \"a/b\", 3]\n");
    assert_eq!(settings.share_extra, ["hook-data"]);
    assert_eq!(warnings.len(), 5, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .all(|w| w.contains("`run.share_extra`") && w.contains("config.toml")),
        "{warnings:?}"
    );
    for name in ["\"\"", "\".\"", "\"..\"", "\"a/b\""] {
        assert!(
            warnings.iter().any(|w| w.contains(name)),
            "{name}: {warnings:?}"
        );
    }
}

#[test]
fn a_share_extra_that_is_neither_a_name_nor_a_list_warns_and_the_next_table_applies() {
    let (settings, warnings) =
        load("[run]\nshare_extra = [\"global\"]\n[provider.claude-code.run]\nshare_extra = 3\n");
    assert_eq!(settings.share_extra, ["global"]);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("`provider.claude-code.run.share_extra`"),
        "{warnings:?}"
    );
}

/// `inspect` of `text` as the `config.toml` of a fresh environment.
fn inspect_as(text: &str, provider: &str) -> Inspection {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.config_dir()).unwrap();
    fs::write(settings::config_path(&env), text).unwrap();
    inspect(&env, &ProviderId::new(provider))
}

/// The registry's key `name`.
fn reg(name: &str) -> &'static Key {
    settings::key(name).unwrap_or_else(|| panic!("no registry key {name}"))
}

/// `name`'s line in `inspection`.
fn state<'a>(inspection: &'a Inspection, name: &str) -> &'a KeyState {
    inspection
        .keys
        .iter()
        .find(|s| s.key.name == name)
        .unwrap_or_else(|| panic!("{name} is not inspected"))
}

fn list(names: &[&str]) -> Value {
    Value::List(models(names))
}

#[test]
fn the_registry_is_section_six_four_s_table_in_order() {
    let rows: Vec<(&str, KeyKind, bool)> = KEYS
        .iter()
        .map(|k| (k.name, k.kind, k.per_provider))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("default_provider", KeyKind::Provider, false),
            (
                "autoswitch.threshold",
                KeyKind::Float {
                    min: 50.0,
                    max: 99.9
                },
                true
            ),
            (
                "autoswitch.interval_seconds",
                KeyKind::Int { min: 15, max: 3600 },
                true
            ),
            (
                "autoswitch.cooldown_seconds",
                KeyKind::Int {
                    min: 0,
                    max: 86_400
                },
                true
            ),
            (
                "autoswitch.hysteresis_pct",
                KeyKind::Float {
                    min: 0.0,
                    max: 50.0
                },
                true
            ),
            (
                "autoswitch.strategy",
                KeyKind::Choice(&["best", "consume-first"]),
                true
            ),
            ("autoswitch.include_api_key_accounts", KeyKind::Bool, true),
            (
                "autoswitch.unhealthy_ticks",
                KeyKind::Int { min: 1, max: 100 },
                true
            ),
            ("autoswitch.models", KeyKind::Models, true),
            (
                "usage.history_retention_days",
                KeyKind::Int { min: 1, max: 3650 },
                false
            ),
            ("statusline.format", KeyKind::Format, true),
            ("run.share_extra", KeyKind::ShareNames, true),
            (
                "ui.color",
                KeyKind::Choice(&["auto", "always", "never"]),
                false
            ),
        ]
    );
}

#[test]
fn a_key_is_found_by_its_dotted_name_and_splits_into_table_and_leaf() {
    for k in KEYS {
        assert_eq!(settings::key(k.name), Some(k));
    }
    assert_eq!(reg("autoswitch.threshold").table(), Some("autoswitch"));
    assert_eq!(reg("autoswitch.threshold").leaf(), "threshold");
    assert_eq!(reg("run.share_extra").table(), Some("run"));
    assert_eq!(reg("default_provider").table(), None);
    assert_eq!(reg("default_provider").leaf(), "default_provider");
    for name in [
        "",
        "threshold",
        "autoswitch",
        "Autoswitch.threshold",
        "autoswitch.threshold ",
        "provider.claude-code.autoswitch.threshold",
    ] {
        assert_eq!(settings::key(name), None, "{name:?}");
    }
}

#[test]
fn every_key_reads_back_its_default_from_the_settings() {
    let d = Settings::default();
    let defaults: Vec<(&str, Value)> = KEYS.iter().map(|k| (k.name, d.value(k))).collect();
    assert_eq!(
        defaults,
        vec![
            ("default_provider", Value::Str("claude-code".into())),
            ("autoswitch.threshold", Value::Float(90.0)),
            ("autoswitch.interval_seconds", Value::Int(60)),
            ("autoswitch.cooldown_seconds", Value::Int(300)),
            ("autoswitch.hysteresis_pct", Value::Float(10.0)),
            ("autoswitch.strategy", Value::Str("best".into())),
            ("autoswitch.include_api_key_accounts", Value::Bool(false)),
            ("autoswitch.unhealthy_ticks", Value::Int(3)),
            ("autoswitch.models", list(&[])),
            ("usage.history_retention_days", Value::Int(180)),
            (
                "statusline.format",
                Value::Str("{account} · 5h {5h}% · 7d {7d}%{stale}".into())
            ),
            ("run.share_extra", list(&[])),
            ("ui.color", Value::Str("auto".into())),
        ]
    );
}

#[test]
fn each_key_says_what_a_valid_value_is() {
    let expect = |name: &str| reg(name).expect();
    assert_eq!(
        expect("default_provider"),
        "must be a provider id of lowercase letters, digits and dashes, such as \"claude-code\""
    );
    assert_eq!(
        expect("autoswitch.threshold"),
        "must be a number from 50 to 99.9"
    );
    assert_eq!(
        expect("autoswitch.interval_seconds"),
        "must be a whole number of seconds from 15 to 3600"
    );
    assert_eq!(
        expect("autoswitch.cooldown_seconds"),
        "must be a whole number of seconds from 0 to 86400"
    );
    assert_eq!(
        expect("autoswitch.hysteresis_pct"),
        "must be a number from 0 to 50"
    );
    assert_eq!(
        expect("autoswitch.strategy"),
        "must be \"best\" or \"consume-first\""
    );
    assert_eq!(
        expect("autoswitch.include_api_key_accounts"),
        "must be true, false, 1, 0, yes or no"
    );
    assert_eq!(
        expect("autoswitch.unhealthy_ticks"),
        "must be a whole number of ticks from 1 to 100"
    );
    assert_eq!(
        expect("autoswitch.models"),
        "must be a model name, a list of model names, or [\"all\"] alone"
    );
    assert_eq!(
        expect("usage.history_retention_days"),
        "must be a whole number of days from 1 to 3650"
    );
    assert!(
        expect("statusline.format")
            .starts_with("must be a non-empty string using only the placeholders {account}, ")
    );
    assert_eq!(
        expect("run.share_extra"),
        "must be an entry name or a list of entry names"
    );
    assert_eq!(
        expect("ui.color"),
        "must be \"auto\", \"always\" or \"never\""
    );
}

/// The numbers at a numeric kind's bounds, and just outside them, as TOML.
fn bounds(kind: KeyKind) -> (Vec<String>, Vec<String>) {
    match kind {
        KeyKind::Float { min, max } => (
            vec![format!("{min:?}"), format!("{max:?}")],
            vec![format!("{:?}", min - 0.1), format!("{:?}", max + 0.1)],
        ),
        KeyKind::Int { min, max } => (
            vec![min.to_string(), max.to_string()],
            vec![(min - 1).to_string(), (max + 1).to_string()],
        ),
        _ => (Vec::new(), Vec::new()),
    }
}

#[test]
fn every_numeric_key_reads_its_bounds_and_defaults_just_outside_them() {
    // §15.2: every registry key's bounds are defaulted by reads. The command line agrees.
    let mut numeric = 0;
    for key in KEYS {
        let (inside, outside) = bounds(key.kind);
        if inside.is_empty() {
            continue;
        }
        numeric += 1;
        let text = |v: &str| format!("[{}]\n{} = {v}\n", key.table().unwrap(), key.leaf());
        for v in &inside {
            let i = inspect_as(&text(v), PROVIDER);
            let s = state(&i, key.name);
            assert_eq!(
                (&s.value, s.source),
                (&key.parse_arg(v).unwrap(), Source::Global),
                "{}: {v}",
                key.name
            );
            assert!(i.warnings.is_empty(), "{}: {v}: {:?}", key.name, i.warnings);
        }
        for v in &outside {
            let i = inspect_as(&text(v), PROVIDER);
            let s = state(&i, key.name);
            assert_eq!(
                (&s.value, s.source),
                (&s.default, Source::Default),
                "{}: {v}",
                key.name
            );
            assert_eq!(i.warnings.len(), 1, "{}: {v}: {:?}", key.name, i.warnings);
            assert!(
                i.warnings[0].contains(&format!("`{}` {} (ignored)", key.name, key.expect())),
                "{:?}",
                i.warnings
            );
            assert_eq!(key.parse_arg(v), Err(key.expect()), "{}: {v}", key.name);
        }
    }
    assert_eq!(
        numeric, 6,
        "threshold, three seconds and ticks, hysteresis, retention"
    );
}

#[test]
fn the_auto_switch_keys_are_read_from_the_provider_s_table_first() {
    let text = "[autoswitch]\ninterval_seconds = 120\ncooldown_seconds = 600\nhysteresis_pct = 5\n\
                strategy = \"best\"\ninclude_api_key_accounts = false\nunhealthy_ticks = 2\n\n\
                [provider.claude-code.autoswitch]\ninterval_seconds = 30\nstrategy = \"consume-first\"\n\
                include_api_key_accounts = true\n";
    let (s, warnings) = load(text);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        (
            s.interval_seconds,
            s.cooldown_seconds,
            s.hysteresis_pct,
            s.strategy,
            s.include_api_key_accounts,
            s.unhealthy_ticks
        ),
        (30, 600, 5.0, Strategy::ConsumeFirst, true, 2),
        "a whole number reads as a float key's value"
    );
    let (s, _) = load_as(text, "fake-agent");
    assert_eq!(
        (s.interval_seconds, s.strategy, s.include_api_key_accounts),
        (120, Strategy::Best, false),
        "another provider's table does not apply"
    );
}

#[test]
fn a_boolean_override_reads_every_spelling_and_an_invalid_one_falls_through() {
    // M3b's reading (§6.4): in the file a boolean is a TOML boolean, the integer 1 or 0, or one
    // of `true/false/1/0/yes/no` as a string. Each of these overrides the global `true`.
    for text in ["false", "0", "\"no\"", "\"false\"", "\"0\""] {
        let (s, warnings) = load(&format!(
            "[autoswitch]\ninclude_api_key_accounts = true\n\
             [provider.claude-code.autoswitch]\ninclude_api_key_accounts = {text}\n"
        ));
        assert!(!s.include_api_key_accounts, "{text}");
        assert!(warnings.is_empty(), "{text}: {warnings:?}");
    }
    // Anything else warns, naming the provider's key, and the global `true` applies.
    for text in ["\"\"", "[false]", "\"No\"", "\"off\"", "2", "0.0"] {
        let (s, warnings) = load(&format!(
            "[autoswitch]\ninclude_api_key_accounts = true\n\
             [provider.claude-code.autoswitch]\ninclude_api_key_accounts = {text}\n"
        ));
        assert!(s.include_api_key_accounts, "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(
            warnings[0].contains(
                "`provider.claude-code.autoswitch.include_api_key_accounts` must be true, false, 1, 0, yes or no"
            ),
            "{warnings:?}"
        );
    }
}

#[test]
fn the_strategy_and_the_colour_read_their_values_exactly() {
    for text in ["\"Best\"", "\"consume_first\"", "\"next-available\"", "1"] {
        let (s, warnings) = load(&format!(
            "[autoswitch]\nstrategy = \"consume-first\"\n\
             [provider.claude-code.autoswitch]\nstrategy = {text}\n"
        ));
        assert_eq!(s.strategy, Strategy::ConsumeFirst, "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(
            warnings[0].contains(
                "`provider.claude-code.autoswitch.strategy` must be \"best\" or \"consume-first\""
            ),
            "{warnings:?}"
        );
    }
    let i = inspect_as("[provider.claude-code.ui]\ncolor = \"never\"\n", PROVIDER);
    assert_eq!(
        state(&i, "ui.color").source,
        Source::Default,
        "ui.color has no provider table"
    );
}

#[test]
fn default_provider_is_read_from_the_top_level_alone() {
    for reader in [PROVIDER, "fake-agent"] {
        let (s, warnings) = load_as("default_provider = \"fake-agent\"\n", reader);
        assert_eq!(
            s.default_provider,
            ProviderId::new("fake-agent"),
            "{reader}"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }
    let i = inspect_as(
        "[provider.claude-code]\ndefault_provider = \"fake-agent\"\n",
        PROVIDER,
    );
    let s = state(&i, "default_provider");
    assert_eq!(
        (&s.value, s.source),
        (&Value::Str("claude-code".into()), Source::Default)
    );
    assert_eq!(i.unknown, ["provider.claude-code.default_provider"]);
}

#[test]
fn unknown_keys_are_sorted_by_dotted_name_however_the_tables_interleave() {
    let i = inspect_as(
        "[provider.claude-code.autoswitch]\nzeta = 1\n\
         [ui]\nalpha = 2\n\
         [provider.claude-code.statusline]\nbeta = 3\n",
        PROVIDER,
    );
    assert_eq!(
        i.unknown,
        [
            "provider.claude-code.autoswitch.zeta",
            "provider.claude-code.statusline.beta",
            "ui.alpha",
        ]
    );
}

#[test]
fn a_malformed_default_provider_falls_back_to_claude_code_with_a_warning() {
    for text in [
        "\"Claude Code\"",
        "\"claude_code\"",
        "\"\"",
        "\"-x\"",
        "5",
        "[\"claude-code\"]",
    ] {
        let (s, warnings) = load(&format!("default_provider = {text}\n"));
        assert_eq!(s.default_provider, ProviderId::new("claude-code"), "{text}");
        assert_eq!(warnings.len(), 1, "{text}: {warnings:?}");
        assert!(
            warnings[0].contains("`default_provider` must be a provider id"),
            "{warnings:?}"
        );
    }
}

#[test]
fn an_empty_provider_list_overrides_a_non_empty_global_one() {
    // Review Focus 2, as a read: `[]` in the provider's table is a value, and it wins.
    let i = inspect_as(
        "[autoswitch]\nmodels = [\"Opus\"]\n[run]\nshare_extra = [\"hook-data\"]\n\n\
         [provider.claude-code.autoswitch]\nmodels = []\n[provider.claude-code.run]\nshare_extra = []\n",
        PROVIDER,
    );
    assert!(i.warnings.is_empty(), "{:?}", i.warnings);
    for name in ["autoswitch.models", "run.share_extra"] {
        let s = state(&i, name);
        assert_eq!(
            (&s.value, s.source),
            (&list(&[]), Source::Provider),
            "{name}"
        );
    }
}

#[test]
fn inspect_reports_each_key_s_value_default_and_source() {
    let text = "[autoswitch]\nthreshold = 80\nmodels = [\"Opus\"]\n\n\
                [provider.claude-code.autoswitch]\nthreshold = 70\n";
    let i = inspect_as(text, PROVIDER);
    assert!(i.exists);
    assert_eq!(
        i.keys.iter().map(|s| s.key.name).collect::<Vec<_>>(),
        KEYS.iter().map(|k| k.name).collect::<Vec<_>>(),
        "every key, in the registry's order"
    );
    let threshold = state(&i, "autoswitch.threshold");
    assert_eq!(
        (&threshold.value, &threshold.default, threshold.source),
        (&Value::Float(70.0), &Value::Float(90.0), Source::Provider)
    );
    let models = state(&i, "autoswitch.models");
    assert_eq!(
        (&models.value, models.source),
        (&list(&["Opus"]), Source::Global)
    );
    let color = state(&i, "ui.color");
    assert_eq!(
        (&color.value, color.source),
        (&Value::Str("auto".into()), Source::Default)
    );
    let other = inspect_as(text, "fake-agent");
    let threshold = state(&other, "autoswitch.threshold");
    assert_eq!(
        (&threshold.value, threshold.source),
        (&Value::Float(80.0), Source::Global)
    );
}

#[test]
fn inspect_and_load_read_every_key_alike() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    fs::create_dir_all(env.config_dir()).unwrap();
    fs::write(
        settings::config_path(&env),
        "default_provider = \"fake-agent\"\n[autoswitch]\nthreshold = 120\ninterval_seconds = 45\n\
         models = \"Fable\"\n[provider.claude-code.autoswitch]\nstrategy = \"consume-first\"\n\
         [provider.claude-code.statusline]\nformat = \"{7d}\"\n[ui]\ncolor = \"never\"\n",
    )
    .unwrap();
    let provider = ProviderId::new(PROVIDER);
    let (loaded, warnings) = Settings::load(&env, &provider);
    let i = inspect(&env, &provider);
    assert_eq!(i.warnings, warnings);
    for s in &i.keys {
        assert_eq!(s.value, loaded.value(s.key), "{}", s.key.name);
    }
}

#[test]
fn inspect_lists_the_keys_the_registry_does_not_know_sorted() {
    let i = inspect_as(
        r#"
colour = "never"
default_provider = "claude-code"

[autoswitch]
threshold = 80
thresold = 80

[usage]
retention = 30

[extra]
anything = 1

[ui]
color = "never"
theme = "dark"

[provider.claude-code]
default_provider = "fake-agent"

[provider.claude-code.autoswitch]
models = ["Fable"]
future = 1

[provider.claude-code.ui]
color = "always"

[provider.fake-agent.statusline]
format = "{5h}"

[provider.fake-agent.run]
share_extra = ["x"]

[provider.fake-agent.usage]
history_retention_days = 3
"#,
        PROVIDER,
    );
    assert_eq!(
        i.unknown,
        [
            "autoswitch.thresold",
            "colour",
            "extra.anything",
            "provider.claude-code.autoswitch.future",
            "provider.claude-code.default_provider",
            "provider.claude-code.ui.color",
            "provider.fake-agent.usage.history_retention_days",
            "ui.theme",
            "usage.retention",
        ]
    );
    assert!(
        i.warnings.is_empty(),
        "an unknown key is no warning: {:?}",
        i.warnings
    );
    assert_eq!(
        state(&i, "ui.color").value,
        Value::Str("never".into()),
        "the provider's ui table is not read"
    );
}

#[test]
fn a_known_table_of_the_wrong_type_warns_and_is_not_unknown() {
    let i = inspect_as(
        "autoswitch = 5\n[[usage]]\nhistory_retention_days = 3\n",
        PROVIDER,
    );
    assert!(i.unknown.is_empty(), "{:?}", i.unknown);
    assert_eq!(i.warnings.len(), 2, "{:?}", i.warnings);
}

#[test]
fn inspecting_a_missing_file_gives_every_default_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let env = Env::for_test(dir.path());
    let i = inspect(&env, &ProviderId::new(PROVIDER));
    assert_eq!(i.path, env.config_dir().join("config.toml"));
    assert_eq!(settings::config_path(&env), i.path);
    assert!(!i.exists);
    assert!(
        i.keys
            .iter()
            .all(|s| s.source == Source::Default && s.value == s.default)
    );
    assert!(i.unknown.is_empty() && i.warnings.is_empty());
    assert!(!env.config_dir().exists(), "inspecting creates nothing");
}

#[test]
fn inspecting_a_corrupt_file_gives_every_default_and_one_warning() {
    let i = inspect_as("[autoswitch\nthreshold = = 3\n", PROVIDER);
    assert!(i.exists);
    assert!(i.keys.iter().all(|s| s.source == Source::Default));
    assert!(i.unknown.is_empty());
    assert_eq!(i.warnings.len(), 1, "{:?}", i.warnings);
    assert!(i.warnings[0].contains("not valid TOML"), "{:?}", i.warnings);
}

#[test]
fn a_boolean_on_the_command_line_is_one_of_six_words() {
    let key = reg("autoswitch.include_api_key_accounts");
    for (raw, want) in [
        ("true", true),
        ("1", true),
        ("yes", true),
        ("false", false),
        ("0", false),
        ("no", false),
    ] {
        assert_eq!(key.parse_arg(raw), Ok(Value::Bool(want)), "{raw}");
    }
    for raw in ["True", "YES", "on", "off", "y", "", " yes", "2"] {
        assert_eq!(
            key.parse_arg(raw),
            Err("must be true, false, 1, 0, yes or no".to_owned()),
            "{raw:?}"
        );
    }
}

#[test]
fn a_number_on_the_command_line_is_taken_as_typed_and_never_clamped() {
    let threshold = reg("autoswitch.threshold");
    assert_eq!(threshold.parse_arg("80"), Ok(Value::Float(80.0)));
    assert_eq!(threshold.parse_arg("99.9"), Ok(Value::Float(99.9)));
    for raw in [
        "100", "49.9", "99.95", "nan", "inf", "-inf", "", "80%", " 80", "eighty",
    ] {
        assert_eq!(
            threshold.parse_arg(raw),
            Err("must be a number from 50 to 99.9".to_owned()),
            "{raw:?}"
        );
    }
    let interval = reg("autoswitch.interval_seconds");
    assert_eq!(interval.parse_arg("60"), Ok(Value::Int(60)));
    for raw in [
        "14",
        "3601",
        "60.0",
        "1e2",
        "",
        "-60",
        "9223372036854775808",
    ] {
        assert_eq!(
            interval.parse_arg(raw),
            Err("must be a whole number of seconds from 15 to 3600".to_owned()),
            "{raw:?}"
        );
    }
}

#[test]
fn a_list_on_the_command_line_is_split_on_commas_and_each_item_trimmed() {
    // Review Focus 2: lists typed the way people type them.
    let models = reg("autoswitch.models");
    assert_eq!(
        models.parse_arg("Fable, opus"),
        Ok(list(&["Fable", "opus"]))
    );
    assert_eq!(models.parse_arg(" Fable "), Ok(list(&["Fable"])));
    assert_eq!(models.parse_arg(""), Ok(list(&[])), "'' is the empty list");
    assert_eq!(models.parse_arg("all"), Ok(list(&["all"])));
    assert_eq!(
        models.parse_arg("Fable,fable,FABLE"),
        Ok(list(&["Fable"])),
        "repeats collapse, ignoring case"
    );
    for raw in ["Fable,,Opus", "Fable,", ",Fable", " ", " , "] {
        let reason = models.parse_arg(raw).unwrap_err();
        assert!(
            reason.starts_with("must be model names separated by commas, with no empty item"),
            "{raw:?}: {reason}"
        );
    }
    for raw in ["all,Fable", "Fable,ALL", " all , Opus"] {
        assert_eq!(
            models.parse_arg(raw),
            Err("must be \"all\" alone, or model names without \"all\"".to_owned()),
            "{raw:?}"
        );
    }
}

#[test]
fn share_extra_on_the_command_line_takes_entry_names_only() {
    let share = reg("run.share_extra");
    assert_eq!(
        share.parse_arg("hook-data, .my-tool,hooks.json"),
        Ok(list(&["hook-data", ".my-tool", "hooks.json"]))
    );
    assert_eq!(
        share.parse_arg("hook-data,hook-data"),
        Ok(list(&["hook-data"]))
    );
    assert_eq!(share.parse_arg(""), Ok(list(&[])));
    for raw in ["a/b", ".", "..", ".tagteam-links.json", "ok,../x"] {
        let reason = share.parse_arg(raw).unwrap_err();
        assert!(
            reason.starts_with("must be entry names of the source home"),
            "{raw:?}: {reason}"
        );
    }
    assert!(
        share
            .parse_arg("a,,b")
            .unwrap_err()
            .starts_with("must be entry names separated by commas")
    );
}

#[test]
fn a_choice_a_format_and_a_provider_on_the_command_line_are_taken_exactly() {
    let strategy = reg("autoswitch.strategy");
    assert_eq!(
        strategy.parse_arg("consume-first"),
        Ok(Value::Str("consume-first".into()))
    );
    for raw in ["Best", "best ", "next-available", ""] {
        assert_eq!(strategy.parse_arg(raw), Err(strategy.expect()), "{raw:?}");
    }
    let color = reg("ui.color");
    assert_eq!(color.parse_arg("never"), Ok(Value::Str("never".into())));
    assert_eq!(color.parse_arg("Never"), Err(color.expect()));
    let format = reg("statusline.format");
    assert_eq!(
        format.parse_arg("{5h}% {model:Fable}"),
        Ok(Value::Str("{5h}% {model:Fable}".into()))
    );
    for raw in ["", "  ", "{nope}", "{5h", "{model: Fable}"] {
        assert_eq!(format.parse_arg(raw), Err(format.expect()), "{raw:?}");
    }
    let provider = reg("default_provider");
    for raw in ["claude-code", "fake-agent", "x1"] {
        assert_eq!(provider.parse_arg(raw), Ok(Value::Str(raw.into())));
    }
    for raw in [
        "",
        "Claude-Code",
        "claude code",
        "claude_code",
        "-x",
        "x-",
        "provider.x",
    ] {
        assert_eq!(provider.parse_arg(raw), Err(provider.expect()), "{raw:?}");
    }
}

#[test]
fn every_refusal_says_what_the_value_must_be() {
    for key in KEYS {
        for raw in [
            "", "nope", "a,,b", "all,x", "{x}", "-1", "1e9", "a/b", "Yes",
        ] {
            if let Err(reason) = key.parse_arg(raw) {
                assert!(
                    reason.starts_with("must be "),
                    "{}: {raw:?}: {reason}",
                    key.name
                );
            }
        }
    }
}

#[test]
fn a_value_survives_the_command_line_and_the_file_unchanged() {
    // What `config get` prints, `config set` takes back; what a write stores, a read reads back.
    let samples = [
        ("default_provider", "fake-agent"),
        ("autoswitch.threshold", "75.5"),
        ("autoswitch.interval_seconds", "120"),
        ("autoswitch.cooldown_seconds", "0"),
        ("autoswitch.hysteresis_pct", "12.25"),
        ("autoswitch.strategy", "consume-first"),
        ("autoswitch.include_api_key_accounts", "yes"),
        ("autoswitch.unhealthy_ticks", "100"),
        ("autoswitch.models", "Fable, Opus"),
        ("usage.history_retention_days", "3650"),
        ("statusline.format", "{account} {5h}%"),
        ("run.share_extra", ""),
        ("ui.color", "always"),
    ];
    assert_eq!(
        samples.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        KEYS.iter().map(|k| k.name).collect::<Vec<_>>(),
        "one sample per key"
    );
    for (name, raw) in samples {
        let key = reg(name);
        let value = key.parse_arg(raw).unwrap();
        assert_eq!(key.parse_arg(&value.display()), Ok(value.clone()), "{name}");
        let mut details = Vec::new();
        assert_eq!(
            key.parse_item(&value.to_item(), &mut |d| details.push(d)),
            Some(value.clone()),
            "{name}"
        );
        assert!(details.is_empty(), "{name}: {details:?}");
    }
}

#[test]
fn a_value_is_written_as_its_toml_type_and_shown_as_its_json_type() {
    assert_eq!(Value::Float(80.0).to_item().as_float(), Some(80.0));
    assert_eq!(Value::Int(60).to_item().as_integer(), Some(60));
    assert_eq!(Value::Bool(true).to_item().as_bool(), Some(true));
    assert_eq!(Value::Str("best".into()).to_item().as_str(), Some("best"));
    let item = list(&["Fable", "Opus"]).to_item();
    let items: Vec<&str> = item
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(items, ["Fable", "Opus"]);
    assert!(list(&[]).to_item().as_array().unwrap().is_empty());

    assert_eq!(Value::Float(80.0).to_json(), json!(80.0));
    assert_eq!(Value::Int(60).to_json(), json!(60));
    assert_eq!(Value::Bool(false).to_json(), json!(false));
    assert_eq!(Value::Str("best".into()).to_json(), json!("best"));
    assert_eq!(list(&["Fable"]).to_json(), json!(["Fable"]));
    assert_eq!(list(&[]).to_json(), json!([]));

    assert_eq!(Value::Float(80.0).display(), "80");
    assert_eq!(Value::Float(99.9).display(), "99.9");
    assert_eq!(list(&["Fable", "Opus"]).display(), "Fable,Opus");
    assert_eq!(list(&[]).display(), "");
    assert_eq!(Source::Provider.as_str(), "provider");
    assert_eq!(Source::Global.as_str(), "global");
    assert_eq!(Source::Default.as_str(), "default");
}

#[test]
fn a_key_resolves_with_or_without_its_provider() {
    let cc = ProviderId::new("claude-code");
    let fake = ProviderId::new("fake-agent");
    let threshold = reg("autoswitch.threshold");
    let cases = [
        ("autoswitch.threshold", None, None),
        ("autoswitch.threshold", Some(&cc), Some(&cc)),
        ("provider.claude-code.autoswitch.threshold", None, Some(&cc)),
        (
            "provider.claude-code.autoswitch.threshold",
            Some(&cc),
            Some(&cc),
        ),
        (
            "provider.fake-agent.autoswitch.threshold",
            None,
            Some(&fake),
        ),
    ];
    for (name, flag, want) in cases {
        let r = resolve(name, flag).unwrap();
        assert_eq!((r.key, r.provider.as_ref()), (threshold, want), "{name}");
    }
    let r = resolve("ui.color", None).unwrap();
    assert_eq!((r.key.name, r.provider), ("ui.color", None));
    let r = resolve("provider.claude-code.run.share_extra", None).unwrap();
    assert_eq!(r.key.name, "run.share_extra");
}

#[test]
fn a_key_that_does_not_resolve_says_why() {
    let cc = ProviderId::new("claude-code");
    let fake = ProviderId::new("fake-agent");
    for name in [
        "",
        "nope",
        "threshold",
        "autoswitch",
        "autoswitch.nope",
        "Autoswitch.threshold",
        "provider",
        "provider.claude-code",
        "provider.claude-code.nope",
        "provider.claude-code.provider.claude-code.autoswitch.threshold",
    ] {
        assert!(
            matches!(resolve(name, None), Err(SettingsError::UnknownKey(n)) if n == name),
            "{name:?}"
        );
    }
    for (name, flag) in [
        ("ui.color", Some(&cc)),
        ("provider.claude-code.ui.color", None),
        ("default_provider", Some(&cc)),
        ("provider.claude-code.default_provider", None),
        ("usage.history_retention_days", Some(&fake)),
    ] {
        assert!(
            matches!(resolve(name, flag), Err(SettingsError::NotPerProvider(_))),
            "{name}"
        );
    }
    assert!(matches!(
        resolve("provider.claude-code.autoswitch.models", Some(&fake)),
        Err(SettingsError::ProviderMismatch { prefix, flag })
            if prefix == "claude-code" && flag == "fake-agent"
    ));
    // A provider that is no id at all is still the caller's to refuse, as `--provider` is.
    let r = resolve("provider.Claude.autoswitch.threshold", None).unwrap();
    assert_eq!(r.provider, Some(ProviderId::new("Claude")));
}

#[test]
fn a_settings_error_tells_the_user_what_to_do() {
    assert_eq!(
        SettingsError::UnknownKey("autoswitch.thresold".into()).to_string(),
        "there is no setting `autoswitch.thresold`; `tagteam config list` shows them all"
    );
    assert_eq!(
        SettingsError::NotPerProvider("ui.color".into()).to_string(),
        "`ui.color` is the same for every provider, so no provider table can set it; name it without `provider.<id>.` and without --provider"
    );
    assert_eq!(
        SettingsError::ProviderMismatch {
            prefix: "claude-code".into(),
            flag: "fake-agent".into()
        }
        .to_string(),
        "`provider.claude-code.…` and `--provider fake-agent` name different providers; give only one of them"
    );
    assert_eq!(
        SettingsError::Invalid {
            key: "autoswitch.threshold".into(),
            reason: "must be a number from 50 to 99.9".into()
        }
        .to_string(),
        "`autoswitch.threshold` must be a number from 50 to 99.9"
    );
}
