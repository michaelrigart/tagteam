//! The TUI's `Host` over the CLI's command path (TUI spec §4.5, §11 "Host bridge"). Each action
//! runs the CLI's own command in-process, as `tests/app.rs` runs them, with its prompts
//! answered by a scripted `Ask` standing in for the TUI's dialogs. `act_foreground`'s session
//! runs a stand-in for `tagteam run` here; the real one, on a terminal, is `tui_pty.rs`'s.

mod common;

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use common::{LOCKED, cc_profile, hold_launch, install_script, live_email, login, seed_home};
use tagteam::app::{self, Context, Io};
use tagteam::cli::Cli;
use tagteam::prompt::Prompter;
use tagteam::tui_host::CliHost;
use tagteam_cc::CLAUDE_CODE;
use tagteam_cc::live::Platform;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::store::{AccountRow, Store};
use tagteam_provider::{Cancel, Env, FakeKeychain};
use tagteam_tui::host::{AccountRef, Action, ActionOutcome, Ask, Host};

const SWITCHED_TO_A: &str = "Switched to a@x.co (position 1).\nClaude Code picks this up within about 30 s; restart it to apply now.\n";
const ADD_STRANGER_FIRST: &str = "Add the current login (stranger@x.co) first?";
const STALE: &str = "tagteam: the selected account changed; nothing was done\n";
const INTERRUPTED: &str = "tagteam: interrupted\n";
/// What `add` says when every endpoint is offline: the oracle could not attribute the login.
const UNVERIFIED: &str = "note: could not verify that the live credential belongs to";

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

/// The TUI's dialogs, scripted. Each `confirm` records its question and default, and takes the
/// next answer. Like the TUI's own `ChannelAsk`, a dialog asked for once the shutdown token is
/// set declines at once, unshown (spec §10.2 step 3). With `quit_while_open`, the person
/// presses `q` while the dialog is open: the shutdown token is set, and the dialog declines.
struct Dialogs {
    answers: VecDeque<bool>,
    asked: Vec<(String, bool)>,
    shutdown: Option<Cancel>,
    quit_while_open: bool,
}

impl Dialogs {
    fn none() -> Dialogs {
        Dialogs::answering(&[])
    }

    fn answering(answers: &[bool]) -> Dialogs {
        Dialogs {
            answers: answers.iter().copied().collect(),
            asked: Vec::new(),
            shutdown: None,
            quit_while_open: false,
        }
    }

    /// Dialogs that watch `shutdown`, as the TUI's do, answering `answers`.
    fn watching(shutdown: &Cancel, answers: &[bool]) -> Dialogs {
        Dialogs {
            shutdown: Some(shutdown.clone()),
            ..Dialogs::answering(answers)
        }
    }
}

impl Ask for Dialogs {
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        if let Some(shutdown) = &self.shutdown
            && shutdown.requested().is_some()
        {
            return false;
        }
        self.asked.push((question.to_owned(), default_yes));
        if self.quit_while_open {
            if let Some(shutdown) = &self.shutdown {
                shutdown.request(libc::SIGINT);
            }
            return false;
        }
        self.answers
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected dialog: {question}"))
    }

    fn choose(&mut self, question: &str, _options: &[String]) -> Option<usize> {
        panic!("unexpected choice: {question}")
    }
}

/// An outcome's exit code and captured output. `NeedsTerminal` fails the test.
fn done(outcome: ActionOutcome) -> (i32, String, String) {
    match outcome {
        ActionOutcome::Done { exit, out, err } => (exit, out, err),
        ActionOutcome::NeedsTerminal => panic!("the action asked for the terminal"),
    }
}

/// A home where Claude Code has run, its in-memory Keychain, and the CLI run in-process on
/// it, as `tests/app.rs`'s `H` runs it.
struct Home {
    dir: tempfile::TempDir,
    env: Env,
    kc: Arc<FakeKeychain>,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        seed_home(&env);
        Home {
            dir,
            env,
            kc: Arc::new(FakeKeychain::new()),
        }
    }

    /// `a@x.co` at position 1 and `b@x.co` at position 2, the live login.
    fn with_two() -> Home {
        let home = Home::new();
        home.login("a@x.co", "rt-a");
        home.cli(&["add"]);
        home.login("b@x.co", "rt-b");
        home.cli(&["add"]);
        home
    }

    /// `a@x.co` stored, and live a login no account holds.
    fn with_unmanaged_login() -> Home {
        let home = Home::new();
        home.login("a@x.co", "rt-a");
        home.cli(&["add"]);
        home.login("stranger@x.co", "rt-s");
        home
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn login(&self, email: &str, rt: &str) {
        login(&self.env, &*self.kc, email, "", rt);
    }

    /// The context the binary builds for this home, its output not a terminal and every
    /// endpoint offline. `env.cancel` is the home's own token: the signal token.
    fn context(&self) -> Context {
        Context {
            env: self.env.clone(),
            keychain: self.kc.clone(),
            vault_keychain: None,
            platform: Platform::MacOs,
            api_base: Some(common::OFFLINE_API_BASE.into()),
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
            shown_warnings: Arc::from(Vec::<String>::new()),
        }
    }

    /// `tagteam <args>` in-process, with nobody to answer a prompt.
    fn cli_run(&self, args: &[&str]) -> (i32, String, String) {
        let cli =
            Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied())).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = app::run(
            cli,
            self.context(),
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
    }

    /// A test's setup step through the CLI, which must succeed.
    fn cli(&self, args: &[&str]) {
        let (code, _, err) = self.cli_run(args);
        assert_eq!(code, 0, "{args:?}: {err}");
    }

    /// The host the TUI would get for this home, and its shutdown token. Nothing here spawns
    /// `Run`, so `exe` names nothing.
    fn host(&self) -> (CliHost, Cancel) {
        self.host_shown(Vec::new())
    }

    /// `host`, for a TUI that printed `shown` at start-up (`tui_cmd`).
    fn host_shown(&self, shown: Vec<String>) -> (CliHost, Cancel) {
        let shutdown = Cancel::new();
        let host = CliHost::new(
            &self.context(),
            shutdown.clone(),
            PathBuf::from("/nonexistent/tagteam"),
            shown,
        );
        (host, shutdown)
    }

    /// The stored accounts, in position order; none without a store.
    fn rows(&self) -> Vec<AccountRow> {
        Store::open_existing(&self.env.data_dir().join("tagteam.db"))
            .unwrap()
            .map(|s| s.accounts(&ProviderId::new(CLAUDE_CODE)).unwrap())
            .unwrap_or_default()
    }

    /// The account at `position`, as the TUI read it.
    fn at(&self, position: u32) -> AccountRef {
        let row = self
            .rows()
            .into_iter()
            .find(|r| r.position == position)
            .unwrap();
        AccountRef {
            id: row.id,
            provider: row.provider,
            position,
        }
    }

    /// The stored account `id`, as it is now.
    fn row(&self, id: &AccountId) -> Option<AccountRow> {
        self.rows().into_iter().find(|r| &r.id == id)
    }
}

#[test]
fn a_switch_runs_the_cli_s_switch_and_captures_its_words() {
    let home = Home::with_two();
    let (host, _) = home.host();
    let mut dialogs = Dialogs::none();
    assert_eq!(
        done(host.act(&Action::Switch(home.at(1)), &mut dialogs)),
        (0, SWITCHED_TO_A.to_owned(), String::new())
    );
    assert!(dialogs.asked.is_empty());
    assert_eq!(live_email(home.root()), "a@x.co");
}

#[test]
fn the_settings_warnings_the_tui_printed_are_not_repeated_in_an_action_s_output() {
    let home = Home::with_two();
    let dir = home.env.config_dir();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), "[autoswitch]\nthreshold = 5\n").unwrap();
    // The CLI itself still says it, once per command.
    let (_, _, err) = home.cli_run(&["list"]);
    assert!(
        err.starts_with("warning: ") && err.lines().count() == 1,
        "{err}"
    );
    // `tui_cmd` passes the host what it printed.
    let printed = err.trim_end().strip_prefix("warning: ").unwrap().to_owned();
    let (host, _) = home.host_shown(vec![printed]);
    let outcome = host.act(&Action::Switch(home.at(1)), &mut Dialogs::none());
    assert_eq!(
        done(outcome),
        (0, SWITCHED_TO_A.to_owned(), String::new()),
        "the warning was printed at start-up"
    );
}

#[test]
fn a_settings_warning_that_appears_after_start_up_is_shown_by_the_action() {
    let home = Home::with_two();
    let (host, _) = home.host();
    let dir = home.env.config_dir();
    fs::create_dir_all(&dir).unwrap();
    // Written in another shell while the TUI runs: the action reads it afresh.
    fs::write(dir.join("config.toml"), "[autoswitch]\nthreshold = 5\n").unwrap();
    let (exit, out, err) = done(host.act(&Action::Switch(home.at(1)), &mut Dialogs::none()));
    assert_eq!((exit, out.as_str()), (0, SWITCHED_TO_A));
    assert!(
        err.starts_with("warning: ") && err.contains("autoswitch.threshold"),
        "{err}"
    );
    // A warning that changed is another warning.
    fs::write(dir.join("config.toml"), "[autoswitch\n").unwrap();
    let (_, _, changed) = done(host.act(&Action::Switch(home.at(2)), &mut Dialogs::none()));
    assert!(
        changed.starts_with("warning: ") && !changed.contains("threshold"),
        "{changed}"
    );
}

#[test]
fn the_start_up_warnings_the_tui_names_are_the_ones_left_out() {
    // The host reads no settings of its own: it leaves out what `tui_cmd` says it printed, and
    // nothing else.
    let home = Home::with_two();
    let dir = home.env.config_dir();
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), "[autoswitch]\nthreshold = 5\n").unwrap();
    let (_, _, warning) = home.cli_run(&["list"]);
    let text = warning
        .trim_end()
        .strip_prefix("warning: ")
        .unwrap()
        .to_owned();
    let exe = PathBuf::from("/nonexistent/tagteam");
    let host = CliHost::new(
        &home.context(),
        Cancel::new(),
        exe.clone(),
        vec![text, "another".into()],
    );
    let outcome = host.act(&Action::Switch(home.at(1)), &mut Dialogs::none());
    assert_eq!(done(outcome).2, "");
    let host = CliHost::new(&home.context(), Cancel::new(), exe, vec![]);
    let outcome = host.act(&Action::Switch(home.at(2)), &mut Dialogs::none());
    assert!(done(outcome).2.starts_with("warning: "));
}

#[test]
fn the_offer_to_add_an_unmanaged_login_is_a_dialog_and_a_yes_adds_it_first() {
    let home = Home::with_unmanaged_login();
    let (host, _) = home.host();
    let mut dialogs = Dialogs::answering(&[true]);
    let (code, out, err) = done(host.act(&Action::Switch(home.at(1)), &mut dialogs));
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        out,
        format!("Added stranger@x.co at position 2.\n{SWITCHED_TO_A}")
    );
    assert_eq!(err, format!("{UNVERIFIED} stranger@x.co\n"));
    assert_eq!(dialogs.asked, [(ADD_STRANGER_FIRST.to_owned(), true)]);
    assert_eq!(home.rows().len(), 2);
    assert_eq!(live_email(home.root()), "a@x.co");
}

#[test]
fn declining_the_offer_cancels_as_the_cli_does_and_adds_nothing() {
    let home = Home::with_unmanaged_login();
    let (host, _) = home.host();
    let mut dialogs = Dialogs::answering(&[false]);
    assert_eq!(
        done(host.act(&Action::Switch(home.at(1)), &mut dialogs)),
        (1, String::new(), "tagteam: cancelled\n".to_owned())
    );
    assert_eq!(dialogs.asked, [(ADD_STRANGER_FIRST.to_owned(), true)]);
    assert_eq!(home.rows().len(), 1);
    assert_eq!(live_email(home.root()), "stranger@x.co");
}

#[test]
fn an_account_in_a_session_is_refused_as_the_cli_refuses_it() {
    let home = Home::with_two();
    let a = home.at(1);
    let (profile, _) = cc_profile(home.root(), a.id.as_str());
    let _session = hold_launch(&profile);
    let (host, _) = home.host();
    for action in [Action::Switch(a.clone()), Action::Remove(a.clone())] {
        assert_eq!(
            done(host.act(&action, &mut Dialogs::none())),
            (
                1,
                String::new(),
                "tagteam: position 1 (a@x.co) is in use by a `tagteam run` session; exit that session, then retry\n"
                    .to_owned()
            ),
            "{action:?}"
        );
    }
    assert_eq!(live_email(home.root()), "b@x.co");
    assert!(home.row(&a.id).is_some());
}

#[test]
fn enable_and_disable_mark_the_account_as_the_cli_does() {
    let home = Home::with_two();
    let (host, _) = home.host();
    let a = home.at(1);
    assert_eq!(
        done(host.act(&Action::Disable(a.clone()), &mut Dialogs::none())),
        (0, "a@x.co is disabled.\n".to_owned(), String::new())
    );
    assert!(home.row(&a.id).unwrap().disabled);
    assert_eq!(
        done(host.act(&Action::Enable(a.clone()), &mut Dialogs::none())),
        (0, "a@x.co is enabled.\n".to_owned(), String::new())
    );
    assert!(!home.row(&a.id).unwrap().disabled);
}

#[test]
fn an_alias_is_set_and_cleared_and_one_that_starts_with_a_dash_meets_the_cli_s_own_check() {
    let home = Home::with_two();
    let (host, _) = home.host();
    let a = home.at(1);
    let set = |alias: &str| Action::SetAlias {
        account: a.clone(),
        alias: alias.to_owned(),
    };
    assert_eq!(
        done(host.act(&set("work"), &mut Dialogs::none())),
        (0, "Position 1 is now work.\n".to_owned(), String::new())
    );
    assert_eq!(home.row(&a.id).unwrap().alias.as_deref(), Some("work"));
    assert_eq!(
        done(host.act(&Action::ClearAlias(a.clone()), &mut Dialogs::none())),
        (
            0,
            "Position 1 has no alias now.\n".to_owned(),
            String::new()
        )
    );
    assert_eq!(home.row(&a.id).unwrap().alias, None);
    // C3.1's `--`: the name reaches the engine as a name, never as a flag, and is refused there.
    assert_eq!(
        done(host.act(&set("-x"), &mut Dialogs::none())),
        (
            1,
            String::new(),
            "tagteam: an alias cannot start with '-'\n".to_owned()
        )
    );
}

#[test]
fn a_move_takes_the_account_to_its_new_position() {
    let home = Home::with_two();
    let (host, _) = home.host();
    let a = home.at(1);
    let to_2 = Action::Move {
        account: a.clone(),
        position: 2,
    };
    assert_eq!(
        done(host.act(&to_2, &mut Dialogs::none())),
        (
            0,
            "a@x.co is now at position 2.\n".to_owned(),
            String::new()
        )
    );
    assert_eq!(home.row(&a.id).unwrap().position, 2);
}

#[test]
fn remove_deletes_the_stored_account_and_claude_code_keeps_its_login() {
    // Core §10.3: removing the live account forgets it; Claude Code's own login stays.
    let home = Home::with_two();
    let (host, _) = home.host();
    let b = home.at(2);
    assert_eq!(
        done(host.act(&Action::Remove(b.clone()), &mut Dialogs::none())),
        (
            0,
            "Removed b@x.co (position 2).\n".to_owned(),
            String::new()
        )
    );
    assert!(home.row(&b.id).is_none());
    assert_eq!(live_email(home.root()), "b@x.co");
}

#[test]
fn remove_refuses_to_delete_what_the_live_login_lives_in() {
    // Core §10.3, §10.5: the environment names account a's profile as Claude Code's secure
    // storage, so deleting the profile would break the live login. Nothing is deleted.
    let home = Home::with_two();
    let a = home.at(1);
    let (_profile, spelling) = cc_profile(home.root(), a.id.as_str());
    let mut ctx = home.context();
    tagteam_cc::env::set_securestorage_dir(&mut ctx.env, Some(spelling.into()));
    let host = CliHost::new(
        &ctx,
        Cancel::new(),
        PathBuf::from("/nonexistent/tagteam"),
        Vec::new(),
    );
    assert_eq!(
        done(host.act(&Action::Remove(a.clone()), &mut Dialogs::none())),
        (
            1,
            String::new(),
            "tagteam: the live login's files are inside what this would delete, since the environment names it; nothing was deleted (tagteam never deletes the live login)\n"
                .to_owned()
        )
    );
    assert!(home.row(&a.id).is_some());
}

#[test]
fn add_stores_the_live_login() {
    let home = Home::new();
    home.login("a@x.co", "rt-a");
    let (host, _) = home.host();
    let add = Action::AddLive {
        provider: ProviderId::new(CLAUDE_CODE),
    };
    assert_eq!(
        done(host.act(&add, &mut Dialogs::none())),
        (
            0,
            "Added a@x.co at position 1.\n".to_owned(),
            format!("{UNVERIFIED} a@x.co\n")
        )
    );
    assert_eq!(home.rows().len(), 1);
}

#[test]
fn p_writes_the_daemon_s_autoswitch_setting_through_config_set_as_the_cli_does() {
    // Spec §7.4, §11: the socket stays read-only (T3); the setting persists in config.toml.
    let home = Home::with_two();
    let (host, _) = home.host();
    let file = home.env.config_dir().join("config.toml");
    let key = "provider.claude-code.daemon.autoswitch";
    for on in [false, true] {
        let action = Action::Autoswitch {
            provider: ProviderId::new(CLAUDE_CODE),
            on,
        };
        assert_eq!(
            done(host.act(&action, &mut Dialogs::none())),
            (
                0,
                format!("Set {key} = {on} in {}.\n", file.display()),
                String::new()
            ),
        );
        let (code, value, _) = home.cli_run(&["config", "get", key]);
        assert_eq!((code, value), (0, format!("{on}\n")));
    }
}

#[test]
fn an_account_moved_elsewhere_is_refused_as_stale_and_nothing_changes() {
    // Spec §4.5: another process moved `a` after the TUI read it; position 1 now holds `b`.
    let home = Home::with_two();
    let a = home.at(1);
    home.cli(&["move", "1", "2"]);
    let (host, _) = home.host();
    for action in [
        Action::Switch(a.clone()),
        Action::Remove(a.clone()),
        Action::Disable(a.clone()),
    ] {
        assert_eq!(
            done(host.act(&action, &mut Dialogs::none())),
            (1, String::new(), STALE.to_owned()),
            "{action:?}"
        );
    }
    assert_eq!(live_email(home.root()), "b@x.co");
    let rows = home.rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| !r.disabled));
    assert_eq!(home.row(&a.id).unwrap().position, 2);
}

#[test]
fn a_locked_keychain_hands_the_action_to_the_terminal_with_nothing_written() {
    // Appendix A.3: macOS asks for the password on the terminal itself. The CLI with nobody to
    // ask refuses outright; the TUI's bridge declines the question unshown, and the action is
    // run again on the terminal (spec §7.3).
    let home = Home::with_two();
    let a = home.at(1);
    home.kc.set_locked(true);
    assert_eq!(
        home.cli_run(&["switch", "1"]),
        (1, String::new(), format!("tagteam: {LOCKED}\n"))
    );
    let (host, _) = home.host();
    for action in [
        Action::Switch(a.clone()),
        Action::Remove(a.clone()),
        Action::AddLive {
            provider: ProviderId::new(CLAUDE_CODE),
        },
    ] {
        let mut dialogs = Dialogs::none();
        assert!(
            matches!(
                host.act(&action, &mut dialogs),
                ActionOutcome::NeedsTerminal
            ),
            "{action:?}"
        );
        assert!(
            dialogs.asked.is_empty(),
            "the unlock question is never a dialog"
        );
    }
    assert_eq!(home.kc.unlock_attempts(), 0);
    assert_eq!(live_email(home.root()), "b@x.co");
    assert_eq!(home.rows().len(), 2);
}

#[test]
fn a_locked_keychain_after_the_shutdown_began_is_the_clis_own_outcome_not_a_hand_over() {
    // Spec §10.2: with the shutdown begun nothing is handed the terminal; the outcome the
    // command captured stands.
    let home = Home::with_two();
    home.kc.set_locked(true);
    let (host, shutdown) = home.host();
    shutdown.request(libc::SIGINT);
    let mut dialogs = Dialogs::watching(&shutdown, &[]);
    let (exit, _, _) = done(host.act(&Action::Switch(home.at(1)), &mut dialogs));
    assert_ne!(exit, 0);
    assert_eq!(home.kc.unlock_attempts(), 0);
    assert_eq!(live_email(home.root()), "b@x.co");
}

#[test]
fn a_shutdown_before_the_prompt_declines_it_unshown_and_the_command_is_interrupted() {
    // Spec §10.2 step 3, §11: the shutdown begins just before the offer would show.
    let home = Home::with_unmanaged_login();
    let (host, shutdown) = home.host();
    shutdown.request(libc::SIGINT);
    let mut dialogs = Dialogs::watching(&shutdown, &[true]);
    assert_eq!(
        done(host.act(&Action::Switch(home.at(1)), &mut dialogs)),
        (130, String::new(), INTERRUPTED.to_owned())
    );
    assert!(dialogs.asked.is_empty());
    assert_eq!(home.rows().len(), 1, "nothing was added");
    assert_eq!(live_email(home.root()), "stranger@x.co");
}

#[test]
fn q_while_a_dialog_is_open_declines_it_and_the_command_is_interrupted() {
    let home = Home::with_unmanaged_login();
    let (host, shutdown) = home.host();
    let mut dialogs = Dialogs {
        quit_while_open: true,
        ..Dialogs::watching(&shutdown, &[])
    };
    assert_eq!(
        done(host.act(&Action::Switch(home.at(1)), &mut dialogs)),
        (130, String::new(), INTERRUPTED.to_owned())
    );
    assert_eq!(dialogs.asked, [(ADD_STRANGER_FIRST.to_owned(), true)]);
    assert_eq!(home.rows().len(), 1);
}

/// A stand-in for `tagteam run`, at `<root>/bin/tagteam`. It records its arguments, then each
/// SIGHUP, SIGINT and SIGTERM it catches, in `<root>/stub.out`, and runs for up to 10 s. A
/// SIGHUP or SIGTERM ends it with 128 + its number, as `run` ends with its session's code.
const STUB: &str = r#"#!/bin/sh
out='@OUT@'
for a in "$@"; do printf 'arg %s\n' "$a" >> "$out"; done
sleeper=
caught() {
  printf 'signal %s\n' "$1" >> "$out"
  if [ "$1" != 2 ]; then
    if [ -n "$sleeper" ]; then kill "$sleeper" 2>/dev/null; fi
    exit $((128 + $1))
  fi
}
trap 'caught 1' HUP
trap 'caught 2' INT
trap 'caught 15' TERM
sleep 10 &
sleeper=$!
echo ready >> "$out"
while kill -0 "$sleeper" 2>/dev/null; do wait "$sleeper"; done
echo exit >> "$out"
"#;

/// Installs `STUB` under `root`: its path, and the file it records in.
fn stub_run(root: &Path) -> (PathBuf, PathBuf) {
    let out = root.join("stub.out");
    let exe = root.join("bin").join("tagteam");
    fs::create_dir_all(root.join("bin")).unwrap();
    install_script(&exe, &STUB.replace("@OUT@", &out.display().to_string()));
    (exe, out)
}

/// The stand-in's arguments, and the signals it caught, by number.
fn recorded(out: &Path) -> (Vec<String>, Vec<String>) {
    let text = fs::read_to_string(out).unwrap_or_default();
    let pick = |key: &str| {
        text.lines()
            .filter_map(|l| l.strip_prefix(key))
            .map(str::to_owned)
            .collect()
    };
    (pick("arg "), pick("signal "))
}

/// Requests `signals` in `token` in turn, 300 ms apart (six of `run::wait`'s looks), once the
/// stand-in is running: a `kill` of the TUI, as its handler records it.
fn signal_once_ready(out: &Path, token: &Cancel, signals: &[i32]) -> thread::JoinHandle<()> {
    let (out, token, signals) = (out.to_owned(), token.clone(), signals.to_vec());
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !fs::read_to_string(&out)
            .unwrap_or_default()
            .contains("ready\n")
        {
            assert!(Instant::now() < deadline, "the stand-in never started");
            thread::sleep(Duration::from_millis(10));
        }
        for signal in signals {
            token.request(signal);
            thread::sleep(Duration::from_millis(300));
        }
    })
}

#[test]
fn run_starts_tagteam_run_on_the_account_and_a_sigterm_is_forwarded_and_kept() {
    // Spec §7.3: the SIGTERM stays recorded, so the TUI shuts down without waiting for Enter.
    let home = Home::with_two();
    let (exe, out) = stub_run(home.root());
    let signal = home.env.cancel.clone();
    let host = CliHost::new(&home.context(), Cancel::new(), exe, Vec::new());
    let a = home.at(1);
    let sender = signal_once_ready(&out, &signal, &[libc::SIGTERM]);
    let (code, _, _) = done(host.act_foreground(&Action::Run(a.clone())));
    sender.join().unwrap();
    assert_eq!(code, 143);
    let (args, signals) = recorded(&out);
    assert_eq!(
        args,
        [
            "--provider",
            "claude-code",
            "--expect-account",
            a.id.as_str(),
            "run",
            "1"
        ]
    );
    assert_eq!(signals, ["15"], "forwarded once");
    assert_eq!(signal.requested(), Some(libc::SIGTERM));
}

#[test]
fn a_sigint_during_the_session_is_the_session_s_and_a_sighup_is_forwarded() {
    // Core §12.5: the terminal sends Ctrl-C to the session itself, so the TUI never forwards
    // it, and spends it.
    let home = Home::with_two();
    let (exe, out) = stub_run(home.root());
    let signal = home.env.cancel.clone();
    let host = CliHost::new(&home.context(), Cancel::new(), exe, Vec::new());
    let sender = signal_once_ready(&out, &signal, &[libc::SIGINT, libc::SIGHUP]);
    let (code, _, _) = done(host.act_foreground(&Action::Run(home.at(1))));
    sender.join().unwrap();
    assert_eq!(code, 129);
    assert_eq!(recorded(&out).1, ["1"], "the SIGINT never reached it");
    assert_eq!(signal.requested(), Some(libc::SIGHUP));
}

#[test]
fn a_signal_recorded_before_the_hand_over_runs_nothing_and_never_waits() {
    let home = Home::with_two();
    let (exe, out) = stub_run(home.root());
    let signal = home.env.cancel.clone();
    let host = CliHost::new(&home.context(), Cancel::new(), exe, Vec::new());
    for n in [libc::SIGTERM, libc::SIGINT] {
        signal.request(n);
        assert_eq!(
            done(host.act_foreground(&Action::Run(home.at(1)))),
            (128 + n, String::new(), INTERRUPTED.to_owned())
        );
        assert_eq!(signal.take(), Some(n), "left for the TUI to act on");
    }
    assert!(!out.exists(), "nothing was spawned");
}

#[test]
fn run_never_runs_inside_the_tui() {
    // In-process, `run` would exec the agent in the TUI's place: it is always suspended.
    let home = Home::with_two();
    let (exe, out) = stub_run(home.root());
    let host = CliHost::new(&home.context(), Cancel::new(), exe, Vec::new());
    let mut dialogs = Dialogs::none();
    assert!(matches!(
        host.act(&Action::Run(home.at(1)), &mut dialogs),
        ActionOutcome::NeedsTerminal
    ));
    assert!(!out.exists());
}
