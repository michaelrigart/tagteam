//! The TUI through the real binary on a pseudo-terminal (TUI spec §11 "End to end"): what it
//! shows, how it leaves the terminal, and how it treats signals with and without a foreground
//! session. `openpty` fails inside a sandbox that denies it, so run these outside one:
//! `cargo test -p tagteam --features test-support --test tui_pty`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use common::pty::{Finished, Pty, Screen};
use common::{
    FakeCall, cmd, fake_claude, fake_claude_calls, live_email, login, path_with, seed_home,
    std_cmd, two_fresh_accounts,
};
use tagteam_core::AccountId;
use tagteam_provider::{Env, FileKeychain, MARKER_FILE, profile_path};

/// How long one step may take before it counts as stuck.
const LONG: Duration = Duration::from_secs(20);
/// R5's line, as the person reads it.
const RETURN: &str = "Press Enter to return to tagteam";

/// `a@x.co` at position 1 and `b@x.co` at 2, the live login (`two_fresh_accounts`), the fake
/// `claude` in `<root>/bin`, and a working directory `<root>/work`. The first row is `a`,
/// whose `run` starts a session.
struct Home {
    dir: tempfile::TempDir,
    a: String,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::tempdir().unwrap();
        let (a, _b) = two_fresh_accounts(dir.path());
        fake_claude(dir.path());
        fs::create_dir_all(dir.path().join("work")).unwrap();
        Home { dir, a }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// The fake `claude`'s record of its runs.
    fn calls(&self) -> PathBuf {
        self.root().join("calls")
    }

    /// `tagteam <args>` as a person starts it: in `<root>/work`, with the fake `claude` first
    /// on `PATH`. It runs until a signal ends it (`FAKE_CLAUDE_HOLD` names a file that never
    /// appears).
    fn command(&self, args: &[&str]) -> Command {
        let mut c = std_cmd(self.root());
        c.args(args)
            .env("PATH", path_with(&self.root().join("bin")))
            .env("FAKE_CLAUDE_OUT", self.calls())
            .env("FAKE_CLAUDE_HOLD", self.root().join("never"))
            .current_dir(self.root().join("work"));
        c
    }

    /// `command(args)` on a terminal.
    fn tui(&self, args: &[&str]) -> Pty {
        Pty::spawn(self.command(args), true)
    }

    /// The session the fake `claude` ran, if one started.
    fn session(&self) -> Option<FakeCall> {
        fake_claude_calls(&self.calls())
            .into_iter()
            .find(|c| c.mode == "session")
    }
}

/// The accounts table, on the alternate screen.
fn accounts(s: &Screen) -> bool {
    s.on_alt() && s.contains("a@x.co") && s.contains("b@x.co")
}

/// The terminal handed back, and the wait for Enter on it (R5).
fn returning(s: &Screen) -> bool {
    !s.on_alt() && s.contains(RETURN)
}

/// Whether the program ever took the alternate screen.
fn entered_alt(output: &[u8]) -> bool {
    output.windows(8).any(|w| w == b"\x1b[?1049h")
}

/// The terminal as the TUI found it: the main screen, canonical mode and echo on.
fn assert_restored(f: &Finished) {
    assert!(
        !f.screen.on_alt(),
        "the TUI never left the alternate screen:\n{}",
        f.screen.text()
    );
    assert_eq!(
        f.lflag & (libc::ICANON | libc::ECHO),
        libc::ICANON | libc::ECHO,
        "raw mode is off"
    );
}

/// Waits for the session `R` started to be running.
fn wait_for_session(home: &Home, tui: &mut Pty) {
    tui.wait_until(LONG, "the session", || {
        home.session().is_some_and(|c| c.ready)
    });
}

#[test]
fn the_emulator_reads_back_text_moves_clears_and_the_alternate_screen() {
    let drawn = b"shell$ \x1b[?1049h\x1b[2J\x1b[1;1Htagteam\x1b[3;5H\x1b[1mbold\x1b[0m\x1b[2;1Hgone\x1b[2;1H\x1b[K";
    let s = Screen::of(drawn, 20, 4);
    assert!(s.on_alt());
    assert_eq!(s.text(), "tagteam\n\n    bold\n");
    let s = Screen::of(&[&drawn[..], b"\x1b[?1049l\r\nback"].concat(), 20, 4);
    assert!(!s.on_alt());
    assert_eq!(s.text(), "shell$\nback\n\n");
    assert_eq!(Screen::of("a界b".as_bytes(), 6, 1).text(), "a界b");
}

#[test]
fn the_first_frame_shows_the_accounts_and_q_leaves_the_terminal_as_it_was() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
    assert_eq!(f.screen.text().trim(), "", "no report: nothing was running");
}

#[test]
fn three_shows_history_and_one_the_accounts_again() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"3");
    tui.wait_for(LONG, "History", |s| {
        s.on_alt() && s.contains("No usage history.")
    });
    tui.type_keys(b"1");
    tui.wait_for(LONG, "the accounts table again", |s| {
        accounts(s) && !s.contains("No usage history.")
    });
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

#[test]
fn tui_history_opens_on_history() {
    let home = Home::new();
    let mut tui = home.tui(&["tui", "history"]);
    tui.wait_for(LONG, "History", |s| {
        s.on_alt() && s.contains("No usage history.")
    });
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

#[test]
fn sigterm_exits_143_with_the_terminal_restored() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.signal(libc::SIGTERM);
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(143), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn sigterm_waits_for_a_switch_inside_its_critical_span_and_reports_it_as_switched() {
    // Spec §10.2: the switch is parked right after its journal row (§9.4 step 6), inside its
    // critical span, when SIGTERM arrives. The TUI restores the terminal, waits for it, and
    // reports it in the CLI's words (core §14.1).
    let home = Home::new();
    let pause = home.root().join("pause");
    fs::create_dir(&pause).unwrap();
    let mut c = home.command(&["tui"]);
    c.env("TAGTEAM_TEST_PAUSE_AT", "after-journal")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut tui = Pty::spawn(c, true);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"s");
    tui.wait_until(LONG, "the switch's journal row", || {
        pause.join("paused").exists()
    });
    tui.signal(libc::SIGTERM);
    thread::sleep(Duration::from_millis(500));
    assert!(
        tui.child.try_wait().unwrap().is_none(),
        "the TUI waits for the switch"
    );
    tui.wait_for(LONG, "the terminal restored while it waits (§10.2)", |s| {
        !s.on_alt()
    });
    assert!(
        tui.child.try_wait().unwrap().is_none(),
        "the TUI still waits for the switch"
    );
    fs::write(pause.join("resume"), b"").unwrap();
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(143), "{}", f.screen.text());
    assert_restored(&f);
    let shown = f.screen.text();
    assert!(
        shown.contains("Switched to a@x.co (position 1)."),
        "{shown}"
    );
    assert!(
        shown.contains("tagteam: interrupted too late to stop: switch had already finished"),
        "{shown}"
    );
    assert_eq!(live_email(home.root()), "a@x.co");
}

#[test]
fn a_bare_tagteam_on_a_terminal_opens_the_tui_and_with_json_runs_list() {
    let home = Home::new();
    let mut tui = home.tui(&[]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0));
    assert_restored(&f);
    // §3: `--json` is `list --json`, on a terminal too.
    let f = home.tui(&["--json"]).finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert!(!entered_alt(&f.output));
    let printed = String::from_utf8_lossy(&f.output);
    assert!(
        printed.contains("\"schemaVersion\":1") && printed.contains("a@x.co"),
        "{printed}"
    );
}

#[test]
fn a_stdin_that_is_not_a_terminal_is_refused_before_the_terminal_is_touched() {
    let home = Home::new();
    let f = Pty::spawn(home.command(&["tui"]), false).finish(LONG);
    assert_eq!(f.status.code(), Some(2));
    assert!(
        f.screen.contains("tagteam: tagteam tui needs a terminal"),
        "{}",
        f.screen.text()
    );
    assert!(!entered_alt(&f.output));
}

#[test]
fn an_unreadable_marker_is_refused_before_the_terminal_is_touched() {
    // Core §12.8: as for every command but `statusline`.
    let home = Home::new();
    let dir = profile_path(
        &Env::for_test(home.root()),
        &AccountId::from_string(&home.a),
    );
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(MARKER_FILE), "{\"format\": \"tagteam-profile\"").unwrap();
    let mut c = home.command(&["tui"]);
    c.env("CLAUDE_CONFIG_DIR", &dir);
    let f = Pty::spawn(c, true).finish(LONG);
    assert_eq!(f.status.code(), Some(1));
    let marker = dir.join(MARKER_FILE).display().to_string();
    assert!(
        String::from_utf8_lossy(&f.output).contains(&marker),
        "{}",
        f.screen.text()
    );
    assert!(!entered_alt(&f.output));
}

#[test]
fn under_r_a_sigterm_to_the_tui_is_forwarded_and_both_exit() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"R");
    wait_for_session(&home, &mut tui);
    tui.signal(libc::SIGTERM);
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(143), "{}", f.screen.text());
    assert_restored(&f);
    let session = home.session().unwrap();
    assert_eq!(
        (session.signals, session.exit),
        (vec![15], Some(143)),
        "forwarded once, through `tagteam run`"
    );
    assert!(
        !String::from_utf8_lossy(&f.output).contains(RETURN),
        "a SIGTERM never waits for Enter"
    );
}

#[test]
fn under_r_ctrl_backslash_ends_the_session_and_the_tui_survives_and_returns() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"R");
    wait_for_session(&home, &mut tui);
    tui.type_keys(b"\x1c");
    tui.wait_for(LONG, "the wait for Enter", returning);
    assert_eq!(
        home.session().unwrap().exit,
        None,
        "SIGQUIT killed the session before it could record an exit"
    );
    tui.type_keys(b"\r");
    tui.wait_for(LONG, "the accounts table again", accounts);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn under_r_ctrl_c_ends_the_session_and_the_tui_carries_on() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"R");
    wait_for_session(&home, &mut tui);
    tui.type_keys(b"\x03");
    tui.wait_for(LONG, "the wait for Enter", returning);
    let session = home.session().unwrap();
    assert_eq!(
        (session.signals, session.exit),
        (vec![2], Some(130)),
        "the terminal's Ctrl-C, once"
    );
    tui.type_keys(b"\r");
    tui.wait_for(LONG, "the accounts table again", accounts);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn run_on_an_account_moved_elsewhere_is_refused_and_launches_nothing() {
    // Spec §4.5, §11: another process moves `a` after the TUI read it, so position 1 holds `b`.
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    // The start-up collection fails offline, which counts as fetched, so the TUI re-reads once
    // (§6.2): the rows go from "no data yet" to "unavailable (pre-send, ...)". Wait for that, so
    // the `move` below is the only change after the TUI's last read.
    tui.wait_for(LONG, "the start-up re-read", |s| {
        s.on_alt() && s.contains("pre-send")
    });
    cmd(home.root()).args(["move", "1", "2"]).assert().success();
    tui.type_keys(b"R");
    let shown = tui.wait_for(LONG, "the wait for Enter", returning);
    assert!(
        shown.contains("tagteam: the selected account changed; nothing was done"),
        "{}",
        shown.text()
    );
    assert!(
        fake_claude_calls(&home.calls()).is_empty(),
        "nothing was launched, not even a login check"
    );
    tui.type_keys(b"\r");
    tui.wait_for(LONG, "the accounts table again", accounts);
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

#[test]
fn a_locked_keychain_runs_the_action_on_the_terminal_and_returns() {
    // Spec §4.5, §7.3: the TUI's dialogs cannot unlock it, so the switch runs again on the
    // terminal, where the CLI asks itself. The file-backed fake Keychain cannot be unlocked.
    let home = Home::new();
    fs::write(home.root().join("keychain").join("LOCKED"), b"").unwrap();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"s");
    tui.wait_for(LONG, "the CLI's unlock question", |s| {
        !s.on_alt() && s.contains("Unlock it now? [Y/n]")
    });
    tui.type_keys(b"y\r");
    let shown = tui.wait_for(LONG, "the wait for Enter", returning);
    assert!(
        shown.contains("tagteam: the login keychain is locked (common over SSH)"),
        "{}",
        shown.text()
    );
    tui.type_keys(b"\r");
    tui.wait_for(LONG, "the accounts table again", accounts);
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
    assert_eq!(live_email(home.root()), "b@x.co", "nothing was switched");
}

/// `tests/logging.rs`'s fixture: `a@x.co` at position 1 and `b@x.co` at 2, `b` live, and then
/// Claude Code rotated b's credential in place. With every endpoint offline, `switch 1` logs
/// the oracle's missing answer at DEBUG.
fn rotated_live_login(root: &Path) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(root)
        .env("TAGTEAM_LOG", "off")
        .arg("add")
        .assert()
        .success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(root)
        .env("TAGTEAM_LOG", "off")
        .arg("add")
        .assert()
        .success();
    login(&env, &kc, "b@x.co", "", "rt-b-rotated");
}

#[test]
fn debug_lines_go_to_the_log_file_and_never_to_the_terminal() {
    // Spec §3, §10.3.
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    rotated_live_login(root);
    let mut c = std_cmd(root);
    c.args(["--debug", "tui"]);
    let mut tui = Pty::spawn(c, true);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"s");
    tui.wait_for(LONG, "the switch's outcome", |s| {
        s.contains("Switched to a@x.co")
    });
    // Two lines open the result panel, which takes `q` (R9); Ctrl-C quits under any overlay.
    tui.type_keys(b"\x03");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    let log = fs::read_to_string(Env::for_test(root).state_dir().join("tagteam.log")).unwrap();
    assert!(log.contains("the profile oracle gave no answer"), "{log}");
    let printed = String::from_utf8_lossy(&f.output);
    assert!(
        !printed.contains("the profile oracle gave no answer") && !printed.contains("DEBUG"),
        "{printed}"
    );
}

#[test]
fn a_lane_that_panics_restores_the_terminal_before_the_panic_shows_and_exits_1() {
    // Spec §10.1, §10.4: the switch panics inside its transaction (the engine's injected
    // panic), so the actions lane dies.
    let home = Home::new();
    let mut c = home.command(&["tui"]);
    c.env("TAGTEAM_TEST_FAIL_AT", "panic:after-journal");
    let mut tui = Pty::spawn(c, true);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"s");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(1), "{}", f.screen.text());
    assert_restored(&f);
    let printed = String::from_utf8_lossy(&f.output);
    let left = printed
        .rfind("\x1b[?1049l")
        .expect("the alternate screen was left");
    let panicked = printed.find("panicked at").expect("the panic is shown");
    assert!(left < panicked, "restored before the panic was printed");
    assert!(
        f.screen
            .contains("tagteam: the TUI's actions lane panicked; the log has the details"),
        "{}",
        f.screen.text()
    );
}
