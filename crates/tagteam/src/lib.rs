use std::ffi::OsString;
use std::io::{IsTerminal, Write};

use clap::error::{ContextKind, ErrorKind};
use clap::{CommandFactory, Parser};

pub mod app;
pub mod auto;
pub mod cli;
mod config_cmd;
mod displaced_cmd;
mod doctor_cmd;
mod history;
mod logfile;
mod logging;
pub mod prompt;
mod purge_cmd;
mod render;
mod root_guard;
mod run;
mod shell_init;
mod signals;
mod statusline;
mod transfer_cmd;

const TEXT_UNDER_JSON: &str = "--help and --version print text; run them without --json";

/// A usage error without the command-line values clap would repeat: any argument may be a
/// secret (`add-token`'s token, whatever its shape), and secrets never reach error output.
/// Only what the command's own definition supplies is kept: the kind, the did-you-mean
/// suggestions and the usage line. Help and version are the command's own text, and pass.
fn without_argument_values(e: clap::Error) -> clap::Error {
    if matches!(
        e.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        return e;
    }
    let mut safe = clap::Error::new(e.kind()).with_cmd(&cli::Cli::command());
    for (kind, value) in e.context() {
        if matches!(
            kind,
            ContextKind::Usage
                | ContextKind::SuggestedArg
                | ContextKind::SuggestedSubcommand
                | ContextKind::SuggestedValue
        ) {
            safe.insert(kind, value.clone());
        }
    }
    safe
}

/// The status bar's own call: `statusline` printing its line (§13.5). Only this call drains
/// stdin, and only this call answers an unusable `HOME` with silence (§5), because a status bar
/// has nowhere to show an error. `--print-config` and the refused `--json` print text that a
/// person reads.
fn status_bar_line(cli: &cli::Cli) -> bool {
    matches!(
        cli.command,
        Some(cli::Command::Statusline {
            print_config: false
        })
    ) && !cli.json
}

/// §5: the root refusal comes before anything is created, and logging creates the log's
/// directory and file. `sudo` keeps the user's `HOME` on macOS, so a root process that got as
/// far as `init` would leave root-owned files in the user's state directory. Returns the exit
/// code when the command is refused, after printing the refusal as `--json` or a person asks.
fn refuse_then_init_logging(
    json: bool,
    refuse: impl FnOnce() -> Result<(), String>,
    init_logging: impl FnOnce(),
    out: &mut impl Write,
    err: &mut impl Write,
) -> Option<i32> {
    if let Err(message) = refuse() {
        if json {
            let _ = writeln!(out, "{}", app::error_json(app::KIND_ROOT, &message));
        } else {
            let _ = writeln!(err, "tagteam: {message}");
        }
        return Some(app::EXIT_ERROR);
    }
    init_logging();
    None
}

/// Runs the CLI and returns the process exit code.
pub fn main_with_args<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    // B.36: `--json` promises one JSON object, but only tagteam's own flag does. An argument
    // after `--` belongs to the agent `run` launches (Decision 8), and is never the flag.
    let json = args
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--json");
    let cli = match cli::Cli::try_parse_from(&args) {
        Ok(c) => c,
        // `--json` promises exactly one JSON object on stdout (B.36), so under it a usage
        // error, and a request for help or the version (which are text), is a JSON usage
        // error. A closed stdout is not a panic.
        Err(e) if json => {
            let message = if e.use_stderr() {
                e.kind().to_string()
            } else {
                TEXT_UNDER_JSON.to_owned()
            };
            let error = app::error_json(app::KIND_USAGE, &message);
            let _ = writeln!(std::io::stdout(), "{error}");
            return app::EXIT_USAGE;
        }
        Err(e) => {
            let code = if e.use_stderr() { app::EXIT_USAGE } else { 0 };
            let _ = without_argument_values(e).print();
            return code;
        }
    };
    // §13.5: Claude Code pipes its session JSON into the status bar command. It is drained
    // here, at the process boundary, rather than in `app::run`: in-process tests drive `run`,
    // and must never read the test runner's stdin. Only a line that will be printed needs it:
    // `--print-config` and the refused `--json` return without Claude Code's JSON ever being
    // read, so a pipe that never closes must not hang them.
    if status_bar_line(&cli) {
        let stdin = std::io::stdin();
        statusline::drain(stdin.lock(), stdin.is_terminal());
    }
    // §5: every default path derives from `HOME`, so an unusable one refuses the command here,
    // before any engine exists, and nothing is created under `/` or the working directory.
    let ctx = match app::Context::from_process() {
        Ok(ctx) => ctx,
        Err(_) if status_bar_line(&cli) => return 0,
        Err(e) => {
            if cli.json {
                let error = app::error_json(e.kind(), &e.to_string());
                let _ = writeln!(std::io::stdout(), "{error}");
            } else {
                let _ = writeln!(std::io::stderr(), "tagteam: {e}");
            }
            return app::EXIT_ERROR;
        }
    };
    // §14.2, Decision 5: logging is a process concern, set up here once and never by
    // `app::run`, which in-process tests drive. After the HOME check, since the log's path
    // derives from HOME, and after the root refusal, which `run` repeats for in-process callers
    // (a refused process never reaches it), and before the command runs.
    let init_logging = || {
        logging::init(
            logging::LogConfig {
                debug: cli.debug,
                color: !cli.no_color && !ctx.no_color_env,
                log_file: ctx.env.log_file(),
                home: ctx.env.home.clone(),
                filter: std::env::var_os(logging::TAGTEAM_LOG),
            },
            &mut std::io::stderr(),
        );
    };
    if let Some(code) = refuse_then_init_logging(
        cli.json,
        root_guard::refuse_root,
        init_logging,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    ) {
        return code;
    }
    // §14.1, at the process boundary like the drain above: in-process tests drive `run` with
    // tokens of their own, and must never change the test runner's signal dispositions. After
    // the drain, so a status bar command stuck on a pipe that never closes still dies on
    // SIGTERM.
    if let Err(e) = signals::install(&ctx.env.cancel) {
        let _ = writeln!(
            std::io::stderr(),
            "warning: could not catch signals, so one stops tagteam where it lands: {e}"
        );
    }
    let mut prompter = prompt::TtyPrompter::new(ctx.env.cancel.clone());
    // Unlocked on purpose: each write locks for itself. Holding the locks across `run` would
    // block any other thread's write to the same stream (a collector thread's tracing event on
    // stderr) while this one waits to join it.
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    app::run(
        cli,
        ctx,
        &mut app::Io {
            out: &mut out,
            err: &mut err,
            prompter: &mut prompter,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn a_refused_root_never_reaches_logging() {
        // The log's directory and file are created by `init`, so a refusal must come first.
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let inited = Cell::new(false);
        let code = refuse_then_init_logging(
            false,
            || Err("no root".into()),
            || inited.set(true),
            &mut out,
            &mut err,
        );
        assert_eq!(code, Some(app::EXIT_ERROR));
        assert!(!inited.get(), "logging was initialised for a refused root");
        assert_eq!(
            (out.as_slice(), err.as_slice()),
            (b"".as_slice(), b"tagteam: no root\n".as_slice())
        );
    }

    #[test]
    fn a_refused_root_under_json_is_one_error_object_on_stdout() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = refuse_then_init_logging(
            true,
            || Err("no root".into()),
            || panic!("logging was initialised for a refused root"),
            &mut out,
            &mut err,
        );
        assert_eq!(code, Some(app::EXIT_ERROR));
        assert!(err.is_empty());
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\"schemaVersion\":1,\"error\":{\"type\":\"root\",\"message\":\"no root\"}}\n"
        );
    }

    #[test]
    fn an_allowed_user_gets_logging_and_no_output() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let inited = Cell::new(false);
        let code =
            refuse_then_init_logging(false, || Ok(()), || inited.set(true), &mut out, &mut err);
        assert_eq!(code, None);
        assert!(inited.get());
        assert!(out.is_empty() && err.is_empty());
    }
}
