//! `tagteam tui`'s refusals and the bare-`tagteam` rule, through the binary and `app::run`
//! (TUI spec §3). The TUI itself, on a terminal, is `tui_pty.rs`'s. Needs
//! `--features test-support`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use clap::Parser;
use common::{cmd, seed_home};
use serde_json::{Value, json};
use tagteam::app::{self, Context, Io};
use tagteam::cli::Cli;
use tagteam::prompt::Prompter;
use tagteam_cc::live::Platform;
use tagteam_provider::{Env, FakeKeychain};

const NEEDS_TERMINAL: &str = "tagteam: tagteam tui needs a terminal\n";

/// §3's JSON refusal.
fn under_json() -> Value {
    json!({"schemaVersion": 1, "error": {"type": "usage",
           "message": "tui has no JSON output; run it without --json"}})
}

/// Nothing was created under HOME (core §5).
fn assert_home_empty(root: &Path) {
    assert!(
        fs::read_dir(root.join("home")).unwrap().next().is_none(),
        "HOME must stay empty"
    );
}

/// A caller that is not a person: any prompt fails the test.
struct Nobody;

impl Prompter for Nobody {
    fn interactive(&self) -> bool {
        false
    }
    fn confirm(&mut self, q: &str, _default_yes: bool) -> bool {
        panic!("unexpected prompt: {q}")
    }
    fn choose(&mut self, q: &str, _options: &[String]) -> Option<usize> {
        panic!("unexpected prompt: {q}")
    }
    fn secret(&mut self, q: &str) -> Option<String> {
        panic!("unexpected prompt: {q}")
    }
}

#[test]
fn tui_under_json_is_one_json_usage_error_and_creates_nothing() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    for args in [&["tui", "--json"][..], &["--json", "tui", "history"]] {
        let out = cmd(d.path())
            .args(args)
            .assert()
            .code(2)
            .stderr("")
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            under_json(),
            "{args:?}"
        );
    }
    assert_home_empty(d.path());
}

#[test]
fn tui_off_a_terminal_is_a_usage_error_and_creates_nothing() {
    // `assert_cmd` gives the binary a null stdin and a piped stdout: neither is a terminal.
    // Under `--debug`, too, stderr holds the refusal and nothing else (§10.3).
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    for args in [&["tui"][..], &["tui", "history"], &["--debug", "tui"]] {
        cmd(d.path())
            .args(args)
            .assert()
            .code(2)
            .stdout("")
            .stderr(NEEDS_TERMINAL);
    }
    assert_home_empty(d.path());
}

#[test]
fn tui_opens_on_accounts_or_history_and_nothing_else_yet() {
    // Part B adds `tokens`.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    cmd(d.path())
        .args(["tui", "tokens"])
        .assert()
        .code(2)
        .stdout("");
}

#[test]
fn a_bare_tagteam_off_a_terminal_still_runs_list() {
    // §3, §12.1: scripts and pipes see no change.
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("home")).unwrap();
    cmd(d.path())
        .assert()
        .success()
        .stdout("No accounts yet. Log in with `claude`, then run `tagteam add`.\n");
    let out = cmd(d.path())
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&out).unwrap()["accounts"],
        json!([])
    );
}

#[test]
fn app_run_refuses_tui_for_an_in_process_caller() {
    // An in-process caller has no terminal to hand over (§3): `tui` is a usage error there.
    let d = tempfile::tempdir().unwrap();
    let env = Env::for_test(d.path());
    seed_home(&env);
    let run = |args: &[&str]| {
        let cli =
            Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied())).unwrap();
        let ctx = Context {
            env: env.clone(),
            keychain: Arc::new(FakeKeychain::new()),
            vault_keychain: None,
            platform: Platform::MacOs,
            api_base: Some(common::OFFLINE_API_BASE.into()),
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = app::run(
            cli,
            ctx,
            &mut Io {
                out: &mut out,
                err: &mut err,
                prompter: &mut Nobody,
            },
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    };
    assert_eq!(run(&["tui"]), (2, String::new(), NEEDS_TERMINAL.to_owned()));
    let (code, out, err) = run(&["tui", "--json"]);
    assert_eq!((code, err.as_str()), (2, ""));
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), under_json());
    assert!(!env.data_dir().exists());
}
