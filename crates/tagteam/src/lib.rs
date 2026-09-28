use std::ffi::OsString;

use clap::Parser;

pub mod app;
pub mod cli;
pub mod prompt;
mod render;
mod root_guard;

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
        // `--json` promises one JSON object on stdout even for a usage error (B.36).
        Err(e) if json && e.use_stderr() => {
            println!(
                "{}",
                app::error_json(app::KIND_USAGE, &e.kind().to_string())
            );
            return app::EXIT_USAGE;
        }
        Err(e) => {
            let code = if e.use_stderr() { app::EXIT_USAGE } else { 0 };
            let _ = e.print();
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
