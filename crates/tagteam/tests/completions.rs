//! `tagteam completions bash|zsh|fish` through the binary (§13.7). Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use clap::CommandFactory;
use common::cmd;
use serde_json::{Value, json};
use tagteam::cli::Cli;
use tagteam_engine::settings::KEYS;

const SHELLS: [&str; 3] = ["bash", "zsh", "fish"];

/// `tagteam completions <shell>`'s script. It says nothing on stderr.
fn script(root: &Path, shell: &str) -> String {
    let out = cmd(root)
        .args(["completions", shell])
        .assert()
        .success()
        .stderr("")
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out).unwrap()
}

/// The words of `script`: its runs of letters, digits, `.`, `-` and `_`. A name a script offers
/// is one of them, whatever the shell's quoting around it.
fn words(script: &str) -> BTreeSet<&str> {
    script
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Every subcommand name and visible alias under `cmd`, and every long flag.
fn definitions(cmd: &clap::Command, names: &mut BTreeSet<String>, flags: &mut BTreeSet<String>) {
    flags.extend(
        cmd.get_arguments()
            .filter_map(|a| a.get_long())
            .map(str::to_owned),
    );
    for sub in cmd.get_subcommands() {
        names.extend(
            sub.get_name_and_visible_aliases()
                .into_iter()
                .map(str::to_owned),
        );
        definitions(sub, names, flags);
    }
}

/// Whether a fish line declares `-l f`, as a whole word.
fn fish_declares_long(script: &str, f: &str) -> bool {
    script.lines().any(|l| {
        l.split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|w| w == ["-l", f])
    })
}

/// Every value an argument under `cmd` offers (hidden ones excepted), recursively.
fn possible_values(cmd: &clap::Command, values: &mut BTreeSet<String>) {
    for arg in cmd.get_arguments() {
        values.extend(
            arg.get_possible_values()
                .into_iter()
                .filter(|v| !v.is_hide_set())
                .map(|v| v.get_name().to_owned()),
        );
    }
    for sub in cmd.get_subcommands() {
        possible_values(sub, values);
    }
}

#[test]
fn each_shell_s_script_offers_every_possible_value_of_every_argument() {
    // Every fixed value completes (§13.7): the ValueEnum ones, the shells, the providers and
    // the boolean words alike.
    let d = tempfile::tempdir().unwrap();
    let mut values = BTreeSet::new();
    possible_values(&Cli::command(), &mut values);
    for expected in [
        "best",
        "next-available",
        "consume-first",
        "claude-code",
        "bash",
        "true",
        "false",
        "1",
        "0",
        "yes",
        "no",
    ] {
        assert!(values.contains(expected), "{expected} is a fixed value");
    }
    for shell in SHELLS {
        let text = script(d.path(), shell);
        let words = words(&text);
        let missing: Vec<&String> = values
            .iter()
            .filter(|v| !words.contains(v.as_str()))
            .collect();
        assert!(missing.is_empty(), "{shell} offers none of {missing:?}");
    }
}

/// Whether `s` is safe to write into a fish `-a "…"` list or a condition string unescaped.
fn fish_safe(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Every name and spelling under `cmd` that tagteam's fish code may write into the script.
fn fish_written(cmd: &clap::Command, names: &mut BTreeSet<String>) {
    for arg in cmd.get_arguments() {
        names.extend(
            arg.get_possible_values()
                .into_iter()
                .filter(|v| !v.is_hide_set())
                .map(|v| v.get_name().to_owned()),
        );
        if !arg.is_positional() {
            names.extend(
                arg.get_short_and_visible_aliases()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| format!("-{s}")),
            );
            names.extend(
                arg.get_long_and_visible_aliases()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|l| format!("--{l}")),
            );
        }
    }
    for sub in cmd.get_subcommands() {
        names.extend(
            sub.get_name_and_visible_aliases()
                .into_iter()
                .map(str::to_owned),
        );
        fish_written(sub, names);
    }
}

#[test]
fn the_fish_script_writes_only_names_that_need_no_quoting() {
    // tagteam's fish lines put values, subcommand names and option spellings between double
    // quotes and into a regex without escaping them. A name with a quote, a space, a `$` or a
    // metacharacter would corrupt the script, so it fails here first.
    let mut names = BTreeSet::new();
    fish_written(&Cli::command(), &mut names);
    names.extend(KEYS.iter().map(|k| k.name.to_owned()));
    names.extend(
        KEYS.iter()
            .filter(|k| k.per_provider)
            .map(|k| format!("provider.claude-code.{}", k.name)),
    );
    assert!(names.contains("config") && names.contains("--provider"));
    let unsafe_names: Vec<&String> = names.iter().filter(|n| !fish_safe(n)).collect();
    assert!(unsafe_names.is_empty(), "{unsafe_names:?}");
    assert!(!fish_safe("a b") && !fish_safe("a$") && !fish_safe("a\"") && !fish_safe("a|b"));
}

#[test]
fn each_shell_s_script_offers_every_command_flag_key_and_provider() {
    // §13.7: commands, flags, provider IDs, settings keys (§6.4) and the other fixed values
    // complete. The expectations come from the same definitions the scripts do.
    let d = tempfile::tempdir().unwrap();
    let (mut names, mut flags) = (BTreeSet::new(), BTreeSet::new());
    definitions(&Cli::command(), &mut names, &mut flags);
    assert!(names.contains("config") && names.contains("get") && names.contains("ls"));
    let mut values: Vec<String> = KEYS.iter().map(|k| k.name.to_owned()).collect();
    values.extend(
        KEYS.iter()
            .filter(|k| k.per_provider)
            .map(|k| format!("provider.claude-code.{}", k.name)),
    );
    values.extend(["claude-code", "bash", "zsh", "fish", "tagteam"].map(str::to_owned));
    for shell in SHELLS {
        let text = script(d.path(), shell);
        assert_eq!(
            text,
            script(d.path(), shell),
            "{shell}: the same script every time"
        );
        let words = words(&text);
        let missing: Vec<&String> = names
            .iter()
            .chain(&values)
            .filter(|w| !words.contains(w.as_str()))
            .collect();
        assert!(missing.is_empty(), "{shell} offers none of {missing:?}");
        // bash and zsh spell a flag `--name`; fish declares it `-l name`.
        let missing: Vec<&String> = flags
            .iter()
            .filter(|f| match shell {
                "fish" => !fish_declares_long(&text, f),
                _ => !words.contains(format!("--{f}").as_str()),
            })
            .collect();
        assert!(
            missing.is_empty(),
            "{shell} offers none of the flags {missing:?}"
        );
    }
}

#[test]
fn bash_completes_a_config_key_a_provider_and_a_shell() {
    // The bash script, sourced, as bash's completion calls it: `_tagteam <command> <word> <previous>`.
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("tagteam.bash");
    fs::write(&path, script(d.path(), "bash")).unwrap();
    let complete = |words: &[&str]| -> Vec<String> {
        let line = words.join(" ");
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(
                "source \"$1\"; shift; COMP_WORDS=(\"$@\"); COMP_CWORD=$(($# - 1)); \
                 _tagteam tagteam \"${COMP_WORDS[COMP_CWORD]}\" \"${COMP_WORDS[COMP_CWORD-1]}\"; \
                 printf '%s\\n' \"${COMPREPLY[@]}\"",
            )
            .arg("bash")
            .arg(&path)
            .args(words)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{line}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let autoswitch: Vec<String> = KEYS
        .iter()
        .filter(|k| k.name.starts_with("auto"))
        .map(|k| k.name.to_owned())
        .collect();
    assert_eq!(complete(&["tagteam", "config", "get", "auto"]), autoswitch);
    assert_eq!(
        complete(&["tagteam", "config", "get", "provider.claude-code.run"]),
        ["provider.claude-code.run.share_extra"]
    );
    assert_eq!(complete(&["tagteam", "--provider", ""]), ["claude-code"]);
    let shells: Vec<String> = complete(&["tagteam", "completions", ""])
        .into_iter()
        .filter(|w| !w.starts_with('-'))
        .collect();
    assert_eq!(shells, ["bash", "zsh", "fish"], "and the flags");
}

/// How each line the fish script adds for a positional starts, up to its pattern's words.
const FISH_POSITIONAL: &str =
    "complete -c tagteam -n \"__fish_tagteam_words | string join ' ' | string match -qr '^";

/// The lines tagteam adds to the fish script, as fish reads them.
struct Fish<'s> {
    /// The options `__fish_tagteam_words` skips the next word after.
    valued: BTreeSet<&'s str>,
    /// Each positional line's pattern, its words joined by spaces, and the values it offers.
    lines: Vec<(&'s str, BTreeSet<&'s str>)>,
}

impl<'s> Fish<'s> {
    fn parse(script: &'s str) -> Self {
        let helper = script
            .split_once("function __fish_tagteam_words\n")
            .expect("the helper is defined")
            .1;
        let valued = helper
            .lines()
            .find_map(|l| l.trim().strip_prefix("else if contains -- $word "))
            .expect("the helper names the options that take a value")
            .split(' ')
            .collect();
        let lines = script
            .lines()
            .filter_map(|l| l.strip_prefix(FISH_POSITIONAL))
            .map(|rest| {
                let (pattern, values) = rest.split_once("\\$'\" -f -a \"").unwrap();
                (pattern, values.trim_end_matches('"').split(' ').collect())
            })
            .collect();
        Fish { valued, lines }
    }

    /// What these lines offer for the word after `line`. As the helper does, this keeps the
    /// words after the first that are neither an option nor the word after one in `valued`. A
    /// line applies when its pattern has as many words, each matching the kept word in its place:
    /// a name, one of `(a|b)`, or any word for `\S+`.
    fn offers(&self, line: &str) -> BTreeSet<&'s str> {
        let mut words = Vec::new();
        let mut skip = false;
        for word in line.split(' ').skip(1) {
            if skip {
                skip = false;
            } else if self.valued.contains(word) {
                skip = true;
            } else if !(word.len() > 1 && word.starts_with('-')) {
                words.push(word);
            }
        }
        // The helper's empty word for an option still waiting for its value.
        if skip {
            words.push("");
        }
        self.lines
            .iter()
            .filter(|(pattern, _)| {
                let parts: Vec<&str> = pattern.split(' ').collect();
                parts.len() == words.len()
                    && parts.iter().zip(&words).all(|(part, word)| {
                        match part.strip_prefix('(').and_then(|p| p.strip_suffix(')')) {
                            Some(names) => names.split('|').any(|n| n == *word),
                            None => (*part == r"\S+" && !word.is_empty()) || part == word,
                        }
                    })
            })
            .flat_map(|(_, values)| values.iter().copied())
            .collect()
    }
}

/// Every spelling of an option under `cmd`, by whether it takes a value.
fn options(cmd: &clap::Command, valued: &mut BTreeSet<String>, flags: &mut BTreeSet<String>) {
    for arg in cmd.get_arguments().filter(|a| !a.is_positional()) {
        let shorts = arg.get_short_and_visible_aliases().unwrap_or_default();
        let longs = arg.get_long_and_visible_aliases().unwrap_or_default();
        let spellings = shorts
            .into_iter()
            .map(|s| format!("-{s}"))
            .chain(longs.into_iter().map(|l| format!("--{l}")));
        if arg.get_action().takes_values() {
            valued.extend(spellings);
        } else {
            flags.extend(spellings);
        }
    }
    for sub in cmd.get_subcommands() {
        options(sub, valued, flags);
    }
}

#[test]
fn fish_completes_a_positional_only_in_its_place() {
    // clap_complete's fish script completes no positional argument: tagteam adds a line for
    // each one with fixed values. A line applies only while the words before the cursor, less
    // options and their values, are exactly its subcommands. No fish runs here, so the test
    // reads the lines as fish would.
    let d = tempfile::tempdir().unwrap();
    let script = script(d.path(), "fish");
    let fish = Fish::parse(&script);
    // The helper skips the value of every option that takes one, global ones included. One
    // list serves every subcommand, since no spelling takes a value in one and none in another.
    let (mut valued, mut flags) = (BTreeSet::new(), BTreeSet::new());
    options(&Cli::command(), &mut valued, &mut flags);
    assert!(valued.contains("-p") && valued.contains("--provider"));
    assert!(valued.is_disjoint(&flags), "{valued:?} {flags:?}");
    assert_eq!(fish.valued, valued.iter().map(String::as_str).collect());
    // `tagteam --provider <TAB>` before any subcommand: clap_complete's own rule fails while
    // an option waits for its value, so a line of tagteam's completes the provider ids wherever
    // the word before the cursor is `-p` or `--provider`.
    let helper = script
        .split_once("function __fish_tagteam_after_provider\n")
        .expect("the provider helper is defined")
        .1;
    assert_eq!(
        helper.lines().take(3).collect::<Vec<_>>(),
        [
            "    set -l words (commandline -opc)",
            "    contains -- $words[-1] -p --provider",
            "end",
        ]
    );
    assert!(
        script.lines().any(|l| l
            == "complete -c tagteam -n \"__fish_tagteam_after_provider\" -f -a \"claude-code\""),
        "the provider ids complete after -p and --provider"
    );
    // Each line names its exact word path.
    let patterns: Vec<&str> = fish.lines.iter().map(|(pattern, _)| *pattern).collect();
    assert_eq!(
        patterns,
        [
            "shell-init",
            "config get",
            "config set",
            "config unset",
            "completions"
        ]
    );
    let mut names: BTreeSet<String> = KEYS.iter().map(|k| k.name.to_owned()).collect();
    names.extend(
        KEYS.iter()
            .filter(|k| k.per_provider)
            .map(|k| format!("provider.claude-code.{}", k.name)),
    );
    let keys: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let shells = BTreeSet::from(["bash", "zsh", "fish"]);
    let nothing = BTreeSet::new();
    for (line, offered) in [
        ("tagteam shell-init", &shells),
        ("tagteam config get", &keys),
        ("tagteam config set", &keys),
        ("tagteam config unset", &keys),
        ("tagteam completions", &shells),
        // Options anywhere, and a global option's value, shift nothing.
        ("tagteam -p claude-code config get", &keys),
        ("tagteam config --provider claude-code --json set", &keys),
        // An option still waiting for its value is completed as that value, never a key.
        ("tagteam config get --provider", &nothing),
        ("tagteam completions -p", &nothing),
        ("tagteam --provider=claude-code completions", &shells),
        // Once the positional is given, it is offered no more.
        ("tagteam config set ui.color", &nothing),
        ("tagteam config unset autoswitch.models", &nothing),
        ("tagteam completions bash", &nothing),
        ("tagteam shell-init zsh", &nothing),
        // Nor before its command, nor under another one.
        ("tagteam", &nothing),
        ("tagteam config", &nothing),
        ("tagteam -p config get", &nothing),
        ("tagteam help config get", &nothing),
    ] {
        assert_eq!(&fish.offers(line), offered, "{line}");
    }
}

#[test]
fn completions_reads_no_settings_and_creates_nothing() {
    // §13.7: no store and no Keychain. A corrupt config.toml draws no warning: the script never
    // reads the settings.
    let d = tempfile::tempdir().unwrap();
    let config = d.path().join("home/.config/tagteam");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("config.toml"), "[autoswitch\n").unwrap();
    fs::create_dir_all(d.path().join("keychain")).unwrap();
    fs::write(d.path().join("keychain/LOCKED"), "").unwrap();
    for shell in SHELLS {
        assert!(!script(d.path(), shell).is_empty(), "{shell}");
    }
    let listed = |dir: &Path| -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect()
    };
    assert_eq!(listed(&d.path().join("home")), [".config"]);
    assert_eq!(listed(&d.path().join("home/.config")), ["tagteam"]);
    assert_eq!(listed(&config), ["config.toml"]);
    assert_eq!(listed(&d.path().join("keychain")), ["LOCKED"]);
}

#[test]
fn completions_under_json_or_for_another_shell_is_a_usage_error() {
    let d = tempfile::tempdir().unwrap();
    let out = cmd(d.path())
        .args(["completions", "bash", "--json"])
        .assert()
        .code(2)
        .stderr("")
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap(),
        json!({"schemaVersion": 1, "error": {"type": "usage",
               "message": "completions prints a script; run it without --json"}})
    );
    cmd(d.path())
        .args(["completions", "powershell"])
        .assert()
        .code(2)
        .stdout("");
    cmd(d.path()).arg("completions").assert().code(2).stdout("");
}
