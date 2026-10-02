use std::ffi::OsString;
use std::io::{IsTerminal, Write};

use clap::error::{ContextKind, ErrorKind};
use clap::{CommandFactory, Parser};

pub mod app;
pub mod auto;
pub mod cli;
mod history;
pub mod prompt;
mod render;
mod root_guard;
mod signals;
mod statusline;

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

/// Runs the CLI and returns the process exit code.
pub fn main_with_args<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let json = args.iter().any(|a| a == "--json");
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
    if matches!(
        cli.command,
        Some(cli::Command::Statusline {
            print_config: false
        })
    ) && !cli.json
    {
        let stdin = std::io::stdin();
        statusline::drain(stdin.lock(), stdin.is_terminal());
    }
    let ctx = app::Context::from_process();
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
