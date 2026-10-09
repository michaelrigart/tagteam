use std::process::ExitCode;

use clap::Parser;

/// The tagteam workspace's development tasks
#[derive(Parser)]
#[command(name = "cargo xtask", bin_name = "cargo xtask")]
enum Xtask {
    /// Check tagteam against the real `claude` and a dedicated test account (spec 15.4)
    Compat(xtask::compat::CompatArgs),
}

fn main() -> ExitCode {
    let Xtask::Compat(args) = Xtask::parse();
    ExitCode::from(xtask::compat::main(args))
}
