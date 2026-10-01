use std::fs;

use tagteam_core::ProviderId;
use tagteam_engine::settings::{
    ColorMode, STATUSLINE_PLACEHOLDERS, Settings, is_statusline_placeholder,
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
    assert_eq!(d.threshold, 90.0);
    assert_eq!(d.models, Vec::<String>::new());
    assert_eq!(d.history_retention_days, 180);
    assert_eq!(
        d.statusline_format,
        "{account} · 5h {5h}% · 7d {7d}%{stale}"
    );
    assert_eq!(d.color, ColorMode::Auto);
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
[autoswitch]
threshold = 75.5
models = ["Fable", "Opus"]

[usage]
history_retention_days = 30

[statusline]
format = "{5h}%"

[ui]
color = "never"
"#,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        settings,
        Settings {
            threshold: 75.5,
            models: models(&["Fable", "Opus"]),
            history_retention_days: 30,
            statusline_format: "{5h}%".to_owned(),
            color: ColorMode::Never,
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
fn keys_this_milestone_does_not_read_are_ignored_without_a_warning() {
    let (settings, warnings) = load(
        "default_provider = \"claude-code\"\n[autoswitch]\ninterval_seconds = 120\nstrategy = \"best\"\nfuture = true\n",
    );
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
        "", "nope", "model", "model:", "Account", "5H", " 5h", "5h ", "{5h}", "5h}",
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
