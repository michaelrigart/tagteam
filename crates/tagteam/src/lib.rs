use std::ffi::OsString;
use std::io::Write;

use clap::error::{ContextKind, ErrorKind};
use clap::{CommandFactory, Parser};

pub mod app;
pub mod cli;
pub mod prompt;
mod render;
mod root_guard;

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
    let mut prompter = prompt::TtyPrompter;
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
    app::run(
        cli,
        app::Context::from_process(),
        &mut app::Io {
            out: &mut out,
            err: &mut err,
            prompter: &mut prompter,
        },
    )
}
