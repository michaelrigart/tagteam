//! The TUI through the real binary on a pseudo-terminal (TUI spec §11 "End to end"): what it
//! shows, how it leaves the terminal, how it treats signals with and without a foreground
//! session, and how it follows a real daemon. `openpty` fails inside a sandbox that denies it,
//! and so does the daemon's socket, so run these outside one:
//! `cargo test -p tagteam --features test-support --test tui_pty`.
#![cfg(feature = "test-support")]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::pty::{Finished, Pty, Screen};
use common::{
    FakeCall, Guarded, cmd, fake_claude, fake_claude_calls, live_email, login, now_epoch_s,
    path_with, seed_home, short_tempdir, std_cmd, turn, two_fresh_accounts, write_transcript,
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
        Home::in_dir(tempfile::tempdir().unwrap())
    }

    /// `new`, in a directory short enough for the daemon's socket (D5.1), with the daemon's
    /// notifications off, so that it starts no `osascript`.
    fn for_daemon() -> Home {
        let home = Home::in_dir(short_tempdir());
        cmd(home.root())
            .args(["config", "set", "notify.enabled", "false"])
            .assert()
            .success();
        home
    }

    fn in_dir(dir: tempfile::TempDir) -> Home {
        let (a, _b) = two_fresh_accounts(dir.path());
        fake_claude(dir.path());
        fs::create_dir_all(dir.path().join("work")).unwrap();
        Home { dir, a }
    }

    /// `tagteam daemon --json` on this home, as a service runs it (stdin closed), once it
    /// switches Claude Code's two accounts (`auto`). It is killed if the test fails.
    fn daemon(&self) -> Guarded {
        let mut c = std_cmd(self.root());
        c.args(["daemon", "--json"]).stdin(Stdio::null());
        let mut daemon = Guarded::spawn(c);
        daemon.wait_for(LONG, "the daemon in auto mode", |out| {
            out.contains(r#""event":"mode""#) && out.contains(r#""mode":"auto""#)
        });
        daemon
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

/// What the TUI writes to ask the terminal its background (OSC 11, then DA1), and the answers.
const QUERY: &[u8] = b"\x1b]11;?\x1b\\\x1b[c";
const OSC_ANSWER: &[u8] = b"\x1b]11;rgb:2828/2c2c/3434\x1b\\";
const DA1_ANSWER: &[u8] = b"\x1b[?62;22c";
/// How long a typed byte takes to be read, so that it is not read together with the next.
const SETTLE: Duration = Duration::from_millis(100);
/// Detection's quiet second (`detect::QUIET`) less a margin: an ending that comes before this
/// is not that give-up, whatever the scheduler does to the events that caused it.
const BEFORE_QUIET: Duration = Duration::from_millis(700);

/// The accounts table, on the alternate screen.
fn accounts(s: &Screen) -> bool {
    s.on_alt() && s.contains("a@x.co") && s.contains("b@x.co")
}

/// `tagteam tui` on a terminal that says it draws 24-bit colour, so the TUI asks its background.
fn asking(home: &Home) -> Pty {
    let mut c = home.command(&["tui"]);
    c.env("COLORTERM", "truecolor");
    Pty::spawn(c, true)
}

fn asked(output: &[u8]) -> usize {
    output.windows(QUERY.len()).filter(|w| *w == QUERY).count()
}

/// A 24-bit foreground colour (`ESC [ 38;2;r;g;b m`): tier 2 draws one, the header's accent among
/// them; tier 1 never does.
const TRUECOLOR: &[u8] = b"38;2;";

fn truecolor(output: &[u8]) -> bool {
    output.windows(TRUECOLOR.len()).any(|w| w == TRUECOLOR)
}

fn history(s: &Screen) -> bool {
    s.on_alt() && s.contains("No usage history.")
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

/// One Claude Code response a minute ago, of 100 input, 20 output and 1,000 cache-read tokens:
/// 1.1K tokens in all.
fn one_response(home: &Home) {
    let at_ms = now_epoch_s() * 1000 - 60_000;
    let counts = [100, 20, 0, 0, 1_000];
    write_transcript(
        home.root(),
        "s-1",
        &[turn("claude-opus-5-5", at_ms, counts)],
    );
}

/// The Tokens screen, its report read from an index that has completed a refresh.
fn token_report(s: &Screen) -> bool {
    s.on_alt()
        && s.contains("[Day] Month Session Block")
        && s.contains("refreshed ")
        && s.contains("1.1K")
}

/// The accounts table, with the TUI following the daemon, which switches automatically.
fn live(s: &Screen) -> bool {
    accounts(s) && s.contains("● auto on")
}

/// SIGTERM, and the daemon's exit 0 (D4.6).
fn stop(daemon: Guarded) {
    daemon.signal(libc::SIGTERM);
    let out = daemon.finish(LONG);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
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
    let queried = b"a\x1b]11;?\x1b\\b\x1b[c\x1b]0;title\x07c";
    assert_eq!(Screen::of(queried, 6, 1).text(), "abc", "no query is text");
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
fn a_burst_of_keys_in_one_write_is_handled_whole() {
    // crossterm reads both keys at once and hands them out one by one: the second waits in
    // its queue, where no wait on the terminal sees it.
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"23");
    tui.wait_for(Duration::from_secs(2), "History", |s| {
        s.on_alt() && s.contains("No usage history.")
    });
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

/// Types `replies` and then `q` in one write, as a late answer to a foreground program's query
/// arrives just ahead of a key: the TUI quits within 2 s. crossterm keeps the replies to
/// itself, and the key waits in its queue behind them.
fn quits_behind(replies: &[u8]) {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(&[replies, b"q"].concat());
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
}

#[test]
fn a_key_behind_a_terminal_reply_is_handled() {
    // A cursor position report.
    quits_behind(b"\x1b[1;1R");
}

#[test]
fn a_key_behind_several_terminal_replies_is_handled() {
    // Each kind crossterm keeps: a cursor position, the keyboard enhancement flags and the
    // primary device attributes, and a second cursor position.
    quits_behind(b"\x1b[1;1R\x1b[?1u\x1b[?62;22c\x1b[1;1R");
}

#[test]
fn a_resize_while_idle_redraws_at_the_new_size() {
    let home = Home::new();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.resize(100, 30);
    tui.wait_for(
        Duration::from_secs(2),
        "the footer on the new last row",
        |s| {
            s.text()
                .split('\n')
                .nth(29)
                .is_some_and(|row| row.starts_with("s switch"))
        },
    );
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

#[test]
fn two_shows_the_token_report_and_one_the_accounts_again() {
    // Spec §5.3, §6.4: the index is refreshed after the first frame, and the report read from
    // it on the report lane.
    let home = Home::new();
    one_response(&home);
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.type_keys(b"2");
    tui.wait_for(LONG, "the token report", token_report);
    tui.type_keys(b"1");
    tui.wait_for(LONG, "the accounts table again", |s| {
        accounts(s) && !s.contains("[Day]")
    });
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn tui_tokens_opens_on_the_token_report() {
    let home = Home::new();
    one_response(&home);
    let mut tui = home.tui(&["tui", "tokens"]);
    tui.wait_for(LONG, "the token report", token_report);
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
fn the_colour_query_is_asked_once_and_an_answer_lets_the_keys_through() {
    let home = Home::new();
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    let answered = tui.output().len();
    tui.type_keys(&[OSC_ANSWER, DA1_ANSWER].concat());
    tui.type_keys(b"3");
    tui.wait_for(Duration::from_secs(2), "History", history);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_eq!(asked(&f.output), 1, "asked once");
    assert!(
        truecolor(&f.output[answered..]),
        "drawn in tier 2 after the answer"
    );
    assert_restored(&f);
}

#[test]
fn a_terminal_that_answers_only_da1_keeps_the_keys_working() {
    let home = Home::new();
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    tui.type_keys(DA1_ANSWER);
    tui.type_keys(b"3");
    tui.wait_for(Duration::from_secs(2), "History", history);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0));
    assert!(!truecolor(&f.output), "tier 1 throughout");
}

#[test]
fn the_first_frame_draws_before_a_late_answer_and_the_next_in_truecolor() {
    // Spec §4.2: the first frame never waits for detection; once the background is reported,
    // the next frame is tier 2's.
    let home = Home::new();
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    tui.wait_for(LONG, "the accounts table", accounts);
    thread::sleep(Duration::from_millis(300));
    let answered = tui.output().len();
    assert!(
        !truecolor(&tui.output()[..answered]),
        "tier 1 until the answer"
    );
    tui.type_keys(&[OSC_ANSWER, DA1_ANSWER].concat());
    tui.wait_for_output(Duration::from_secs(2), "a truecolor frame", TRUECOLOR);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert!(!truecolor(&f.output[..answered]), "only after the answer");
}

#[test]
fn a_silent_terminal_is_given_up_after_a_second() {
    let home = Home::new();
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    thread::sleep(Duration::from_millis(1_200));
    tui.type_keys(b"3");
    tui.wait_for(Duration::from_secs(2), "History", history);
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

#[test]
fn without_colorterm_or_with_no_color_nothing_is_asked() {
    let home = Home::new();
    let mut plain = home.command(&["tui"]);
    plain.env_remove("COLORTERM");
    let mut no_colour = home.command(&["tui"]);
    no_colour.env("COLORTERM", "truecolor").env("NO_COLOR", "1");
    for cmd in [plain, no_colour] {
        let mut tui = Pty::spawn(cmd, true);
        tui.wait_for(LONG, "the accounts table", accounts);
        thread::sleep(Duration::from_millis(300));
        assert_eq!(asked(&tui.output()), 0, "nothing was asked");
        tui.type_keys(b"q");
        assert_eq!(tui.finish(LONG).status.code(), Some(0));
    }
}

/// Types `before`, `key` and `after`, each in a read of its own, at a TUI that has asked its
/// background, then `2`. Tokens shows and History, which `3` would have opened, never did: the
/// key was dropped by detection, not read.
fn drops_the_key_in(before: &[u8], key: &[u8], after: &[u8]) {
    let home = Home::new();
    one_response(&home);
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    for bytes in [before, key, after] {
        if !bytes.is_empty() {
            tui.type_keys(bytes);
            thread::sleep(SETTLE);
        }
    }
    tui.type_keys(b"2");
    tui.wait_for(Duration::from_secs(2), "the token report", token_report);
    assert_eq!(tui.first_shown(history), None, "the key reached the keys");
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

#[test]
fn a_key_before_the_colour_answer_is_dropped() {
    drops_the_key_in(b"", b"3", &[OSC_ANSWER, DA1_ANSWER].concat());
}

#[test]
fn a_key_between_the_two_answers_is_dropped() {
    drops_the_key_in(OSC_ANSWER, b"3", DA1_ANSWER);
}

#[test]
fn an_answer_split_across_reads_is_read_whole() {
    let home = Home::new();
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    let (first, rest) = OSC_ANSWER.split_at(12);
    tui.type_keys(first);
    thread::sleep(Duration::from_millis(100));
    tui.type_keys(rest);
    tui.type_keys(DA1_ANSWER);
    tui.type_keys(b"3");
    tui.wait_for(Duration::from_secs(2), "History", history);
    tui.type_keys(b"q");
    assert_eq!(tui.finish(LONG).status.code(), Some(0));
}

/// An answer whose digits would show Tokens or History, and whose `b` would switch, if any of it
/// leaked. The DA1 answer that follows it is short.
const ANSWER: &[u8] = b"\x1b]11;rgb:2b3b/2b3b/2b3b\x1b\\";
const SHORT_DA1: &[u8] = b"\x1b[?62c";
/// What the banner says while the keys are held.
const HELD: &str = "the terminal sent an unfinished colour report";

fn tokens(s: &Screen) -> bool {
    s.on_alt() && s.contains("[Day]")
}

fn help(s: &Screen) -> bool {
    s.on_alt() && (s.contains(" Help ") || s.contains("keys and marks"))
}

fn held(s: &Screen) -> bool {
    s.on_alt() && s.contains(HELD)
}

fn removing(s: &Screen) -> bool {
    s.on_alt() && s.contains("Remove ")
}

/// A TUI that asked its background, and heard nothing for over a second: detection has given up.
fn given_up(home: &Home) -> Pty {
    let mut tui = asking(home);
    tui.wait_for_output(LONG, "the query", QUERY);
    thread::sleep(Duration::from_millis(1_300));
    tui
}

/// The accounts table is still what shows, as if nothing was typed, then `3`: History shows, and
/// `unseen` never did before it. A burst of leaked keys can end on History before a frame draws,
/// so the table is looked at before the `3`, as well as for the screens `unseen` names.
fn then_history(tui: &mut Pty, unseen: &[&dyn Fn(&Screen) -> bool]) {
    thread::sleep(SETTLE);
    let screen = tui.screen();
    assert!(
        accounts(&screen) && !history(&screen),
        "a key of the reply was read:\n{}",
        screen.text()
    );
    tui.type_keys(b"3");
    tui.wait_for(Duration::from_secs(2), "History", history);
    for seen in unseen {
        assert_eq!(tui.first_shown(seen), None, "a key of the reply was read");
    }
}

fn quit(tui: Pty) {
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn an_answer_after_the_give_up_never_reaches_the_keys() {
    let home = Home::new();
    one_response(&home);
    let mut tui = given_up(&home);
    tui.type_keys(&[ANSWER, SHORT_DA1].concat());
    then_history(&mut tui, &[&tokens]);
    assert_eq!(live_email(home.root()), "b@x.co");
    quit(tui);
}

#[test]
fn a_delayed_split_answer_never_reaches_the_keys() {
    let home = Home::new();
    one_response(&home);
    let mut tui = given_up(&home);
    tui.type_keys(b"\x1b");
    thread::sleep(Duration::from_millis(500));
    tui.type_keys(b"]11;rgb:2b3b/2b3b/2b3b\x07");
    then_history(&mut tui, &[&tokens]);
    assert_eq!(live_email(home.root()), "b@x.co");
    quit(tui);
}

#[test]
fn a_delayed_split_da1_answer_never_reaches_the_keys() {
    let home = Home::new();
    one_response(&home);
    let mut tui = given_up(&home);
    tui.type_keys(b"\x1b");
    thread::sleep(Duration::from_millis(500));
    tui.type_keys(b"[?62;22c");
    then_history(&mut tui, &[&help, &tokens]);
    quit(tui);
}

#[test]
fn a_delayed_split_answer_leaves_an_open_dialog_open() {
    let home = Home::new();
    let mut tui = given_up(&home);
    tui.type_keys(b"x");
    tui.wait_for(Duration::from_secs(2), "the remove question", removing);
    tui.type_keys(b"\x1b");
    thread::sleep(Duration::from_millis(500));
    tui.type_keys(b"]11;rgb:2b3b/2b3b/2b3b\x07");
    thread::sleep(Duration::from_millis(300));
    assert!(
        removing(&tui.screen()),
        "the Esc did not close the question"
    );
    tui.type_keys(b"n");
    tui.wait_for(Duration::from_secs(2), "the question closed", |s| {
        accounts(s) && !removing(s)
    });
    quit(tui);
}

#[test]
fn after_an_answer_esc_acts_at_once() {
    let home = Home::new();
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    tui.type_keys(&[ANSWER, SHORT_DA1].concat());
    tui.type_keys(b"?");
    tui.wait_for(Duration::from_secs(2), "help", help);
    tui.type_keys(b"\x1b");
    // A lone Esc held for the next key is never released by time: any bound under the quiet
    // second tells the two apart.
    tui.wait_for(Duration::from_millis(900), "help closed", |s| {
        accounts(s) && !help(s)
    });
    quit(tui);
}

#[test]
fn a_truncated_answer_whose_end_comes_late_holds_the_keys_until_it_ends() {
    let home = Home::new();
    one_response(&home);
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    tui.type_keys(b"\x1b]11;rgb:2b3b/");
    tui.wait_for(Duration::from_secs(2), "the banner", held);
    thread::sleep(Duration::from_millis(1_500));
    tui.type_keys(b"3");
    thread::sleep(SETTLE);
    assert!(held(&tui.screen()), "the keys are still held");
    tui.type_keys(b"2b3b/2b3b\x1b\\");
    tui.wait_for(Duration::from_secs(2), "the banner gone", |s| !held(s));
    tui.type_keys(b"2");
    tui.wait_for(Duration::from_secs(2), "Tokens", tokens);
    assert_eq!(tui.first_shown(history), None, "the `3` was read");
    assert_eq!(live_email(home.root()), "b@x.co");
    quit(tui);
}

#[test]
fn a_typed_ahead_esc_does_not_unhold_a_truncated_answer() {
    let home = Home::new();
    one_response(&home);
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    tui.type_keys(b"\x1b\x1b]11;rgb:2b3b/");
    tui.wait_for(Duration::from_secs(2), "the banner", held);
    thread::sleep(Duration::from_millis(1_500));
    tui.type_keys(b"3");
    thread::sleep(SETTLE);
    assert!(held(&tui.screen()), "the keys are still held");
    tui.type_keys(b"2b3b/2b3b\x1b\\");
    tui.wait_for(Duration::from_secs(2), "the banner gone", |s| !held(s));
    tui.type_keys(b"2");
    tui.wait_for(Duration::from_secs(2), "Tokens", tokens);
    assert_eq!(tui.first_shown(history), None, "the `3` was read");
    assert_eq!(live_email(home.root()), "b@x.co");
    quit(tui);
}

#[test]
fn a_reply_after_a_typed_alt_bracket_and_the_give_up_never_reaches_the_keys() {
    let home = Home::new();
    one_response(&home);
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    tui.type_keys(b"\x1b[");
    thread::sleep(Duration::from_millis(1_300));
    tui.type_keys(b"\x1b]11;rgb:2b3b/2b3b/2b3b\x07");
    then_history(&mut tui, &[&tokens]);
    assert_eq!(live_email(home.root()), "b@x.co");
    quit(tui);
}

#[test]
fn a_typed_backslash_does_not_end_a_held_answer() {
    let home = Home::new();
    one_response(&home);
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    tui.type_keys(b"\x1b]11;rgb:2b3b/");
    tui.wait_for(Duration::from_secs(2), "the banner", held);
    tui.type_keys(b"\\");
    thread::sleep(SETTLE);
    tui.type_keys(b"3");
    thread::sleep(SETTLE);
    assert!(held(&tui.screen()), "a bare backslash is no terminator");
    tui.type_keys(b"2b3b/2b3b\x1b\\");
    tui.wait_for(Duration::from_secs(2), "the banner gone", |s| !held(s));
    tui.type_keys(b"2");
    tui.wait_for(Duration::from_secs(2), "Tokens", tokens);
    assert_eq!(tui.first_shown(history), None, "the `3` was read");
    quit(tui);
}

#[test]
fn a_da1_answer_cut_before_its_c_holds_the_keys_until_the_c() {
    let home = Home::new();
    one_response(&home);
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    tui.type_keys(&[ANSWER, b"\x1b[?6"].concat());
    tui.wait_for(Duration::from_secs(2), "the banner", held);
    thread::sleep(Duration::from_millis(1_300));
    tui.type_keys(b"2;22c");
    tui.wait_for(Duration::from_secs(2), "the banner gone", |s| !held(s));
    then_history(&mut tui, &[&tokens]);
    quit(tui);
}

#[test]
fn an_answer_that_never_ends_still_lets_ctrl_c_quit() {
    let home = Home::new();
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    tui.type_keys(b"\x1b]11;rgb:2b3b/");
    tui.wait_for(Duration::from_secs(2), "the banner", held);
    tui.type_keys(b"\x03");
    let f = tui.finish(Duration::from_secs(2));
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn sigterm_while_the_terminal_ignores_the_colour_query_exits_at_once() {
    let home = Home::new();
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    tui.signal(libc::SIGTERM);
    let f = tui.finish(BEFORE_QUIET);
    assert_eq!(f.status.code(), Some(143), "{}", f.screen.text());
    assert_restored(&f);
}

#[test]
fn ctrl_c_while_the_terminal_ignores_the_colour_query_quits_at_once() {
    let home = Home::new();
    let mut tui = asking(&home);
    tui.wait_for_output(LONG, "the query", QUERY);
    tui.type_keys(b"\x03");
    let f = tui.finish(BEFORE_QUIET);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
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
    // The selection starts on the live login, `b` (T3-23): `k` selects `a`.
    tui.type_keys(b"ks");
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
    tui.wait_for(LONG, "the start question", |s| {
        s.on_alt() && s.contains("in this terminal?")
    });
    tui.type_keys(b"\r");
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
    tui.wait_for(LONG, "the start question", |s| {
        s.on_alt() && s.contains("in this terminal?")
    });
    tui.type_keys(b"\r");
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
    tui.wait_for(LONG, "the start question", |s| {
        s.on_alt() && s.contains("in this terminal?")
    });
    tui.type_keys(b"\r");
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
fn under_r_ctrl_c_ends_the_session_and_a_running_index_refresh_goes_on() {
    // Spec §7.3, §11: the lanes watch only the shutdown token, so the terminal's Ctrl-C for the
    // session leaves the index's catch-up running, parked here with its lock held; it then
    // completes, and the report shows it.
    let home = Home::new();
    one_response(&home);
    let pause = home.root().join("pause");
    fs::create_dir(&pause).unwrap();
    let mut c = home.command(&["tui"]);
    c.env("TAGTEAM_TEST_PAUSE_AT", "refresh-tokens-locked")
        .env("TAGTEAM_TEST_PAUSE_DIR", &pause);
    let mut tui = Pty::spawn(c, true);
    tui.wait_for(LONG, "the accounts table", accounts);
    tui.wait_until(LONG, "the index's refresh, parked", || {
        pause.join("paused").exists()
    });
    tui.type_keys(b"R");
    tui.wait_for(LONG, "the start question", |s| {
        s.on_alt() && s.contains("in this terminal?")
    });
    tui.type_keys(b"\r");
    wait_for_session(&home, &mut tui);
    tui.type_keys(b"\x03");
    tui.wait_for(LONG, "the wait for Enter", returning);
    tui.type_keys(b"\r");
    tui.wait_for(LONG, "the accounts table again", accounts);
    let db = Env::for_test(home.root()).data_dir().join("tagteam.db");
    let parked: Option<i64> = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT refreshed_at FROM token_state", [], |r| r.get(0))
        .unwrap();
    assert_eq!(parked, None, "the refresh was parked when Ctrl-C was sent");
    fs::write(pause.join("resume"), b"").unwrap();
    tui.type_keys(b"2");
    let shown = tui.wait_for(LONG, "the token report", token_report);
    assert!(
        !shown.contains("not refreshed"),
        "the refresh was not interrupted: {}",
        shown.text()
    );
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
    // (§6.2): the rows go from "not checked yet" to "usage check failed (pre-send, ...)". Wait
    // for that, so the `move` below is the only change after the TUI's last read.
    tui.wait_for(LONG, "the start-up re-read", |s| {
        s.on_alt() && s.contains("pre-send")
    });
    cmd(home.root()).args(["move", "1", "2"]).assert().success();
    // The selection starts on the live login, `b` (T3-23): `k` selects `a`.
    tui.type_keys(b"kR");
    tui.wait_for(LONG, "the start question", |s| {
        s.on_alt() && s.contains("in this terminal?")
    });
    tui.type_keys(b"\r");
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
    // Spec §4.5, §7.3: the TUI's dialogs cannot unlock it, so it asks, and on Enter the switch
    // runs again on the terminal, where the CLI asks itself. The file-backed fake Keychain cannot be unlocked.
    let home = Home::new();
    fs::write(home.root().join("keychain").join("LOCKED"), b"").unwrap();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "the accounts table", accounts);
    // The selection starts on the live login, `b` (T3-23): `k` selects `a`.
    tui.type_keys(b"ks");
    tui.wait_for(LONG, "the hand-over question", |s| {
        s.on_alt() && s.contains("Your login keychain is locked.")
    });
    tui.type_keys(b"\r");
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
    // The selection starts on the live login, `b` (T3-23): `k` selects `a`.
    tui.type_keys(b"ks");
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
    // The selection starts on the live login, `b` (T3-23): `k` selects `a`.
    tui.type_keys(b"ks");
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

#[test]
fn under_r_ctrl_c_ends_the_session_and_the_daemon_connection_stays_live() {
    // Spec §7.3, §11: the live thread watches no token, so the session's Ctrl-C leaves the
    // daemon connection as it was.
    let home = Home::for_daemon();
    let daemon = home.daemon();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "live mode", live);
    tui.type_keys(b"R");
    tui.wait_for(LONG, "the start question", |s| {
        s.on_alt() && s.contains("in this terminal?")
    });
    tui.type_keys(b"\r");
    wait_for_session(&home, &mut tui);
    tui.type_keys(b"\x03");
    tui.wait_for(LONG, "the wait for Enter", returning);
    tui.type_keys(b"\r");
    tui.wait_for(LONG, "the accounts table, still live", live);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    assert_restored(&f);
    stop(daemon);
}

#[test]
fn p_pauses_the_daemon_s_auto_switch_and_resumes_it() {
    // Spec §7.4: through `config set`; the header turns as the daemon's `status` does.
    let home = Home::for_daemon();
    let daemon = home.daemon();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "live mode", live);
    tui.type_keys(b"P");
    tui.wait_for(LONG, "the question", |s| {
        s.contains("Pause auto for Claude Code?")
    });
    tui.type_keys(b"y");
    let confirmed = Instant::now();
    tui.wait_for(LONG, "the pause", |s| s.contains("‖ auto paused"));
    let config = fs::read_to_string(home.root().join("home/.config/tagteam/config.toml")).unwrap();
    assert!(config.contains("autoswitch = false"), "{config}");
    // The `y` closed the dialog: an action key within 300 ms of it is dropped (§9.5).
    thread::sleep(Duration::from_millis(350).saturating_sub(confirmed.elapsed()));
    tui.type_keys(b"P");
    tui.wait_for(LONG, "the question", |s| {
        s.contains("Resume auto for Claude Code?")
    });
    tui.type_keys(b"y");
    tui.wait_for(LONG, "auto-switch again", live);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    stop(daemon);
}

#[test]
fn a_daemon_that_stops_leaves_the_tui_on_its_timer_until_the_next_one_answers() {
    // Spec §8: `stopping` names its reason; the TUI refreshes on its own timer, and reconnects
    // with backoff, reading every view again.
    let home = Home::for_daemon();
    let daemon = home.daemon();
    let mut tui = home.tui(&["tui"]);
    tui.wait_for(LONG, "live mode", live);
    stop(daemon);
    tui.wait_for(LONG, "timer mode", |s| {
        accounts(s) && s.contains("! daemon stopping (signal)")
    });
    let daemon = home.daemon();
    tui.wait_for(LONG, "live mode again", live);
    tui.type_keys(b"q");
    let f = tui.finish(LONG);
    assert_eq!(f.status.code(), Some(0), "{}", f.screen.text());
    stop(daemon);
}
