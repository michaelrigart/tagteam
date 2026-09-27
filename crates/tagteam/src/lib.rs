use std::ffi::OsString;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "tagteam",
    version,
    about = "Multi-account switcher for AI coding agent CLIs"
)]
struct Cli {}

/// Runs the CLI and returns the process exit code.
pub fn main_with_args<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match Cli::try_parse_from(args) {
        Ok(_) => 0,
        Err(e) => {
            let code = if e.use_stderr() { 2 } else { 0 };
            let _ = e.print();
            code
        }
    }
}
