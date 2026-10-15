//! TUI spec §4.5: the TUI's `Host`, over the CLI's own command path. An action runs as
//! `tagteam <args>` runs: parsed by the CLI's `clap` definition and run by `app::run`. Inside
//! the TUI (`act`) its output is captured and its prompts go to the TUI's dialogs. Once the TUI
//! has handed the terminal back (`act_foreground`), it runs on the terminal itself, and `Run` is
//! a `tagteam run` process of its own (§7.3).

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::Parser;
use tagteam_provider::Cancel;
use tagteam_provider::process::{SpawnSpec, exit_code, spawn_session};
use tagteam_tui::host::{Action, ActionOutcome, Ask, Host};

use crate::app::{self, Context, Io};
use crate::cli::Cli;
use crate::prompt::{self, Prompter, TtyPrompter};
use crate::run;

/// R5, TUI spec §7.3 step 4: what the terminal shows once a foreground action is done.
pub const RETURN_PROMPT: &str = "Press Enter to return to tagteam";

/// The TUI's `Host` (TUI spec §4.5).
pub struct CliHost {
    /// The in-TUI actions' context. `env.cancel` is the shutdown token, so a `q` reaches them
    /// and a Ctrl-C meant for a foreground action does not (§2).
    inside: Context,
    /// The foreground actions' context. `env.cancel` is the signal token.
    foreground: Context,
    /// The binary `Run` spawns (§7.3 step 3).
    exe: PathBuf,
}

impl CliHost {
    /// `ctx` is the process's context, whose token is the signal token. `exe` is the binary
    /// that `Run` spawns as `<exe> --provider P --expect-account ID run N`:
    /// `std::env::current_exe()` in production.
    pub fn new(ctx: &Context, shutdown: Cancel, exe: PathBuf) -> CliHost {
        let mut inside = ctx.clone();
        inside.env.cancel = shutdown;
        // Captured, so never a terminal; `--no-color` keeps it plain either way.
        inside.stdout_terminal = false;
        CliHost {
            inside,
            foreground: ctx.clone(),
            exe,
        }
    }

    /// The signal token (§2).
    fn signal(&self) -> &Cancel {
        &self.foreground.env.cancel
    }

    /// §7.3 step 3 for `Run`: `tagteam run` as a child of the TUI, in its process group, which
    /// is the terminal's foreground group, so Ctrl-C and Ctrl-\ reach the session from the
    /// terminal itself (core §14.1). It is waited for as `run` waits for `claude` (core §12.5):
    /// SIGTERM and SIGHUP are forwarded as they arrive, and a SIGINT is the session's. A
    /// forwarded signal is recorded again once the child is reaped, so the TUI shuts down after
    /// it (§7.3, §10.2).
    fn run_session(&self, action: &Action) -> ActionOutcome {
        let spec = SpawnSpec {
            program: self.exe.clone(),
            args: action.args().into_iter().map(OsString::from).collect(),
            ..SpawnSpec::default()
        };
        let mut child = match spawn_session(&spec, None) {
            Ok(child) => child,
            Err(e) => {
                let err = format!("tagteam: could not start {}: {e}\n", self.exe.display());
                let _ = io::stderr().write_all(err.as_bytes());
                return ActionOutcome::Done {
                    exit: app::EXIT_ERROR,
                    out: String::new(),
                    err,
                };
            }
        };
        let (status, forwarded) = run::wait(&mut child, self.signal());
        if let Some(signal) = forwarded {
            self.signal().request(signal);
        }
        let exit = match status {
            Ok(status) => exit_code(status),
            Err(e) => {
                let _ = writeln!(
                    io::stderr(),
                    "tagteam: lost track of the session's exit status: {e}"
                );
                app::EXIT_ERROR
            }
        };
        ActionOutcome::Done {
            exit,
            out: String::new(),
            err: String::new(),
        }
    }

    /// §4.5: any other action on the terminal, as the CLI runs it: its own prompter, and the
    /// real stdout and stderr.
    fn run_here(&self, action: &Action) -> ActionOutcome {
        let cli = match parse(action) {
            Ok(cli) => cli,
            Err(err) => {
                let _ = io::stderr().write_all(err.as_bytes());
                return ActionOutcome::Done {
                    exit: app::EXIT_USAGE,
                    out: String::new(),
                    err,
                };
            }
        };
        let mut prompter = TtyPrompter::new(self.signal().clone());
        let (mut out, mut err) = (io::stdout(), io::stderr());
        let exit = app::run(
            cli,
            self.foreground.clone(),
            &mut Io {
                out: &mut out,
                err: &mut err,
                prompter: &mut prompter,
            },
        );
        ActionOutcome::Done {
            exit,
            out: String::new(),
            err: String::new(),
        }
    }
}

impl Host for CliHost {
    fn act(&self, action: &Action, ask: &mut dyn Ask) -> ActionOutcome {
        // In-process, `run` would exec the agent in the TUI's place: it always runs suspended.
        if action.needs_terminal() {
            return ActionOutcome::NeedsTerminal;
        }
        let cli = match parse(action) {
            Ok(cli) => cli,
            Err(err) => {
                return ActionOutcome::Done {
                    exit: app::EXIT_USAGE,
                    out: String::new(),
                    err,
                };
            }
        };
        let mut prompter = Bridge {
            ask,
            needs_terminal: false,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let exit = app::run(
            cli,
            self.inside.clone(),
            &mut Io {
                out: &mut out,
                err: &mut err,
                prompter: &mut prompter,
            },
        );
        if prompter.needs_terminal {
            return ActionOutcome::NeedsTerminal;
        }
        ActionOutcome::Done {
            exit,
            out: String::from_utf8_lossy(&out).into_owned(),
            err: String::from_utf8_lossy(&err).into_owned(),
        }
    }

    fn act_foreground(&self, action: &Action) -> ActionOutcome {
        // A signal that arrived before the terminal was handed over is the TUI's: nothing
        // runs, and the TUI shuts down or carries on as its loop decides (§10.2).
        if let Some(signal) = self.signal().requested() {
            return ActionOutcome::Done {
                exit: 128 + signal,
                out: String::new(),
                err: "tagteam: interrupted\n".to_owned(),
            };
        }
        let outcome = match action {
            Action::Run(_) => self.run_session(action),
            _ => self.run_here(action),
        };
        wait_for_enter(self.signal());
        outcome
    }
}

/// `action`'s arguments, parsed by the CLI's own definition (§4.5). They are the TUI's own, so
/// a failure is a bug. It is reported as the usage error it is, without the arguments' values
/// (`lib::without_argument_values`'s rule).
fn parse(action: &Action) -> Result<Cli, String> {
    let args = std::iter::once("tagteam".to_owned()).chain(action.args());
    Cli::try_parse_from(args).map_err(|e| format!("tagteam: {}\n", e.kind()))
}

/// R5, §7.3 step 4. A SIGINT recorded by now was the action's, typed at its prompt or in its
/// session: it is spent, so it does not end the wait at once. Only a SIGINT is spent, in one
/// step (`take_if`), so a SIGTERM or SIGHUP that replaced it stays recorded. A SIGTERM or
/// SIGHUP ends the TUI (§10.2), so nothing is waited for. Enter, the end of input, or a signal
/// ends the wait.
fn wait_for_enter(signal: &Cancel) {
    signal.take_if(libc::SIGINT);
    if signal.requested().is_some() {
        return;
    }
    let mut err = io::stderr();
    let _ = write!(err, "{RETURN_PROMPT}");
    let _ = err.flush();
    let _ = prompt::read_terminal_line(signal);
}

/// §4.5: the CLI's prompts, sent to the TUI's dialogs. A person answers there, so the command
/// prompts as it would on a terminal. The keychain unlock is the exception (Appendix A.3): macOS
/// asks for the password on the terminal itself, so the question is declined unasked and
/// noted, the command fails before touching anything, and `act` hands the action to the
/// terminal. No action asks for a secret; one that did is declined.
struct Bridge<'a> {
    ask: &'a mut dyn Ask,
    needs_terminal: bool,
}

impl Prompter for Bridge<'_> {
    fn interactive(&self) -> bool {
        true
    }

    fn reaches_terminal(&self) -> bool {
        false
    }

    fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        if question == app::UNLOCK_QUESTION {
            self.needs_terminal = true;
            return false;
        }
        self.ask.confirm(question, default_yes)
    }

    fn choose(&mut self, question: &str, options: &[String]) -> Option<usize> {
        self.ask.choose(question, options)
    }

    fn secret(&mut self, _question: &str) -> Option<String> {
        None
    }
}
