//! `cargo xtask compat` (§15.4): the interop checks that need the real `claude` and a real
//! test account, run against a `test-support` build of `tagteam` driven as a binary.

pub mod capture;
pub mod checks;
pub mod ctx;
pub mod daemon;
pub mod guard;
pub mod keychain;
pub mod layout;
pub mod login;
pub mod registry;
pub mod report;
pub mod run;
pub mod store;
pub mod sys;
pub mod version;

#[derive(clap::Args, Debug, Clone, PartialEq, Eq)]
#[command(args_conflicts_with_subcommands = true)]
pub struct CompatArgs {
    #[command(subcommand)]
    pub command: Option<CompatCommand>,
    /// Leave the scratch homes, the profiles and their Keychain items for inspection
    #[arg(long)]
    pub keep: bool,
    /// Also check that claude reads tagteam-written items over SSH to HOST, this Mac
    #[arg(long, value_name = "HOST")]
    pub ssh: Option<String>,
    /// Also check the lock check on a locked login keychain (asks you questions)
    #[arg(long = "locked-keychain")]
    pub locked_keychain: bool,
    /// Run only CHECK; repeat for more
    #[arg(long = "only", value_name = "CHECK")]
    pub only: Vec<String>,
    /// After a full pass, write claude's version to compat/tested-cc-version
    #[arg(long)]
    pub bless: bool,
}

#[derive(clap::Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum CompatCommand {
    /// Log the dedicated test account in once, and keep it in the compat store
    Login,
}

/// The run's exit code: 0 when every check passed, 1 when one failed, 2 when the harness
/// itself failed (a usage error included).
pub fn main(args: CompatArgs) -> u8 {
    match args.command {
        Some(CompatCommand::Login) => login::login(),
        None => run::run(&args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        task: Task,
    }

    #[derive(clap::Subcommand)]
    enum Task {
        Compat(CompatArgs),
    }

    fn parse(args: &[&str]) -> Result<CompatArgs, clap::Error> {
        let Task::Compat(a) =
            Cli::try_parse_from(std::iter::once("xtask").chain(args.iter().copied()))?.task;
        Ok(a)
    }

    #[test]
    fn the_command_line_reads_as_section_15_4_spells_it() {
        let a = parse(&[
            "compat",
            "--keep",
            "--only",
            "auth-status",
            "--only",
            "config-lock",
            "--bless",
        ])
        .unwrap();
        assert!(a.keep && a.bless && a.command.is_none());
        assert_eq!(a.only, ["auth-status", "config-lock"]);
        let a = parse(&["compat", "--ssh", "localhost", "--locked-keychain"]).unwrap();
        assert_eq!(a.ssh.as_deref(), Some("localhost"));
        assert!(a.locked_keychain);
        assert_eq!(
            parse(&["compat", "login"]).unwrap().command,
            Some(CompatCommand::Login)
        );
        assert!(
            parse(&["compat", "--keep", "login"]).is_err(),
            "login takes no run options"
        );
    }
}
