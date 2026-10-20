use std::ffi::OsString;

use tagteam_engine::EngineError;
use tagteam_engine::run::{RunPlan, RunRequest};
use tagteam_provider::process::{SpawnSpec, exec_command};

use crate::app::App;
use crate::app::failure::Failure;

impl App<'_, '_> {
    /// §12.1 and §12.5: `run`. Whatever fails before the launch command starts is the
    /// command's own error, as the `--json` envelope too. Once `claude` has run, its exit code
    /// is the command's (B.63).
    pub(in crate::app) fn run_agent(
        &mut self,
        account: Option<String>,
        require_session: bool,
        args: Vec<OsString>,
    ) -> Result<i32, Failure> {
        let account = match &account {
            Some(input) => Some(self.resolve(input)?.id),
            None => None,
        };
        let cwd = std::env::current_dir().map_err(EngineError::Io)?;
        let request = RunRequest {
            account,
            provider: self.provider_flag.clone(),
            require_session,
            cwd: cwd.clone(),
            args: args.clone(),
        };
        let (account, launch) = match self.engine.plan_run(&request)? {
            RunPlan::Plain { spec, warning } => return self.run_plain(&spec, warning),
            RunPlan::Session {
                account, launch, ..
            } => (account, launch),
        };
        // Appendix A.3: a session reads the vault and the profile's Keychain item. Plain
        // `claude` reads neither, so it never waits on this check (§12.7).
        self.ensure_unlocked()?;
        let (launched, launch) = match self.engine.launch(&account, &launch, &cwd) {
            Ok(launched) => (launched, launch),
            // Decision 14: the target changed under the launch's locks. Plan once more and act
            // on that plan; a second change is this command's error, never a loop.
            Err(EngineError::TargetChanged { why }) => {
                let _ = writeln!(self.io.err, "warning: {why}");
                match self.engine.plan_run(&request)? {
                    RunPlan::Plain { spec, warning } => return self.run_plain(&spec, warning),
                    RunPlan::Session {
                        account, launch, ..
                    } => (self.engine.launch(&account, &launch, &cwd)?, launch),
                }
            }
            Err(e) => return Err(e.into()),
        };
        // Decision 18: the environment warnings are among them, computed by the engine.
        for w in &launched.warnings {
            let _ = writeln!(self.io.err, "warning: {w}");
        }
        let cancel = self.engine.cancel();
        // §12.3 "Every launch is checked"; a launch that bootstrapped skips it. A
        // refused or interrupted check spends any pending signal on the exit handling
        // (`abandon`), which then runs to completion. An interruption exits 128 + n, and a
        // refusal keeps its own error.
        if let Err(e) = self.engine.check_login(&launched, &launch, &cwd) {
            let e = crate::run::abandon(&self.engine, launched, cancel, &mut *self.io.err, e);
            return Err(e.into());
        }
        Ok(crate::run::run_session(
            &self.engine,
            launched,
            &launch,
            &args,
            &cwd,
            cancel,
            &mut *self.io.err,
        )?)
    }

    /// §12.1's plain `claude`: `exec`, after the warning a plan may carry and the token's last
    /// look. Past that look, signals are plain `claude`'s own. It returns only when the `exec`
    /// failed.
    fn run_plain(&mut self, spec: &SpawnSpec, warning: Option<String>) -> Result<i32, Failure> {
        if let Some(w) = warning {
            let _ = writeln!(self.io.err, "warning: {w}");
        }
        if let Some(signal) = self.engine.cancel().requested() {
            return Err(EngineError::Interrupted(signal).into());
        }
        let _ = self.io.out.flush();
        let _ = self.io.err.flush();
        Err(crate::run::exec_failed(spec, exec_command(spec)).into())
    }
}

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Command};

    use super::OsString;

    #[test]
    fn run_hands_everything_after_the_double_dash_to_the_agent() {
        use clap::Parser;
        // Decision 8: tagteam's options come before `--`, and nothing after it is tagteam's.
        let cli = Cli::try_parse_from([
            "tagteam",
            "run",
            "2",
            "--require-session",
            "--",
            "--json",
            "-p",
            "x",
            "--",
        ])
        .unwrap();
        assert!(!cli.json, "an agent's --json is not tagteam's");
        assert_eq!(cli.provider, None, "nor is its -p");
        let Some(Command::Run {
            account,
            require_session,
            args,
        }) = cli.command
        else {
            panic!("not parsed as run");
        };
        assert_eq!(account.as_deref(), Some("2"));
        assert!(require_session);
        assert_eq!(args, ["--json", "-p", "x", "--"].map(OsString::from));
    }

    #[test]
    fn run_s_own_options_stay_its_own_before_the_double_dash() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["tagteam", "--json", "run", "-p", "claude-code", "--", "hi"])
                .unwrap();
        assert!(cli.json);
        assert_eq!(cli.provider.as_deref(), Some("claude-code"));
        let Some(Command::Run {
            account,
            require_session,
            args,
        }) = cli.command
        else {
            panic!("not parsed as run");
        };
        assert_eq!((account, require_session), (None, false));
        assert_eq!(args, [OsString::from("hi")]);
        assert!(
            Cli::try_parse_from(["tagteam", "run", "2", "hi"]).is_err(),
            "an agent argument needs the --"
        );
    }
}
