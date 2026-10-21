use std::sync::Arc;

use serde_json::Value;
use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::purge::KEYCHAIN_ORPHANS_WITH_PROVIDER;
use tagteam_engine::service::ServiceKind;
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::keychain::ReadCounts;
use tagteam_provider::{Keychain, LockState, Platform};

use crate::cli::{Cli, Command};
use crate::{daemon_cmd, root_guard};

use early::{answer_completions, refuse_tui, run_shell_init, run_statusline};
use failure::{Failure, KIND_INTERRUPTED, KIND_KEYCHAIN_LOCKED};

mod commands;
mod context;
mod dispatch;
mod early;
mod failure;

pub(crate) use context::{
    ColorEnv, command_settings, engine_for, fallback_provider, locate_run_shell, registry,
};
pub use context::{Context, Io};
pub use failure::KIND_STALE_SELECTION;
pub(crate) use failure::{EXIT_ERROR, EXIT_USAGE, KIND_ROOT, KIND_USAGE, error_json, fail_to};

/// §13.1: an interrupted command exits 128 + the signal (130 after SIGINT).
const EXIT_SIGNAL_BASE: i32 = 128;

/// Appendix A.3. The default keychain is the login keychain, so the hint names its file.
pub(crate) const UNLOCK_QUESTION: &str =
    "The login keychain is locked (common over SSH). Unlock it now?";
const KEYCHAIN_LOCKED: &str = "the login keychain is locked (common over SSH); run `security unlock-keychain ~/Library/Keychains/login.keychain-db`, then retry";

const INTERRUPTED: &str = "interrupted";

/// A provider's display name, or its ID for one this build does not register.
fn display_name(engine: &Engine, provider: &ProviderId) -> String {
    engine
        .provider(provider)
        .map_or_else(|_| provider.to_string(), |p| p.display_name().to_owned())
}

struct App<'a, 'b> {
    engine: Engine,
    json: bool,
    color: ColorEnv,
    provider_flag: Option<ProviderId>,
    /// `--expect-account`: the only account `resolve` may return (§10.4).
    expect_account: Option<AccountId>,
    /// The settings warnings `run` printed from the engine's read (§6.4).
    settings_warnings: Vec<String>,
    /// The Keychain Appendix A.3's lock check asks: macOS only, since Linux has none.
    keychain: Option<Arc<dyn Keychain>>,
    /// `Context::platform`: the daemon's notification backend depends on it (D6.2).
    platform: Platform,
    /// The read counts of each Keychain instance the command uses, which the daemon's
    /// keychain backoff compares (D4.1). Empty on Linux, which has no Keychain to back off for.
    keychain_counts: Vec<Arc<ReadCounts>>,
    io: &'a mut Io<'b>,
}

/// How a command ended (§13.1, §14.1).
enum Ended {
    /// It finished with this exit code; its output, and any error, are written.
    Code(i32),
    /// It stopped at a cancellation point after this signal; nothing is reported yet.
    Interrupted(i32),
    /// `run`'s child ran and exited with this code (§12.5, B.63). No late notice follows it:
    /// every signal while the child ran was the child's, and exit handling reports for itself.
    Child(i32),
}

/// Runs one command and returns its exit code (§13.1). A signal the command met at a
/// cancellation point ends it with 128 + the signal and the `interrupted` error;
/// one it never met leaves its output and exit code alone and is reported on stderr as too
/// late (§14.1). `run` ends with its child's code once the child has run.
pub fn run(cli: Cli, ctx: Context, io: &mut Io<'_>) -> i32 {
    let json = cli.json;
    let name = cli.command.as_ref().map_or("list", command_name);
    // §11.4 and D4.6: a signal is how the `auto` loop and the daemon end, so it is never too
    // late for them.
    let stops_on_a_signal = matches!(
        cli.command,
        Some(Command::Auto { once: false, .. } | Command::Daemon { action: None })
    );
    let cancel = ctx.env.cancel.clone();
    match run_command(cli, ctx, io) {
        Ended::Interrupted(signal) => {
            io.fail(json, KIND_INTERRUPTED, INTERRUPTED);
            EXIT_SIGNAL_BASE + signal
        }
        Ended::Code(code) => {
            // A SIGPIPE is the reader leaving, not a request to stop: there is nobody to tell
            // that the command had already finished.
            let too_late = cancel
                .requested()
                .is_some_and(|signal| signal != libc::SIGPIPE);
            if too_late && !stops_on_a_signal {
                let _ = writeln!(
                    io.err,
                    "tagteam: interrupted too late to stop: {name} had already finished"
                );
            }
            code
        }
        Ended::Child(code) => code,
    }
}

fn run_command(cli: Cli, ctx: Context, io: &mut Io<'_>) -> Ended {
    let json = cli.json;
    if let Err(msg) = root_guard::refuse_root() {
        return Ended::Code(io.fail(json, KIND_ROOT, &msg));
    }
    // §13.5: the status bar's fast path, before anything else is built.
    if let Some(Command::Statusline { print_config }) = &cli.command {
        return Ended::Code(run_statusline(
            ctx,
            io,
            json,
            cli.no_color,
            cli.provider,
            *print_config,
        ));
    }
    // TUI spec §3: the TUI runs only from the binary, on its terminal (`tui_cmd`). An
    // in-process caller has none to hand over.
    if let Some(Command::Tui { .. }) = &cli.command {
        return refuse_tui(io, json);
    }
    // §12.7: the wrapper needs only the registered providers, so it reads no store and no
    // settings. Its first action is the marker check (`locate_run_shell`): under a marker that
    // cannot be read it refuses, as every command but `statusline` does (§12.8).
    if let Some(Command::ShellInit { shell }) = &cli.command {
        return Ended::Code(match locate_run_shell(&ctx) {
            Ok(located) => {
                run_shell_init(&located.registry, io, json, cli.provider.as_deref(), *shell)
            }
            Err(e) => io.fail(json, e.kind(), &e.to_string()),
        });
    }
    // §13.7: a script from the command definitions alone. It reads no settings, no store and no
    // Keychain, so it warns about nothing and creates nothing. Its first action is the marker
    // check (`locate_run_shell`): an unreadable marker refuses every command but `statusline`
    // (§12.8).
    if let Some(Command::Completions { shell }) = &cli.command {
        return match locate_run_shell(&ctx) {
            Ok(_) => answer_completions(io, json, *shell),
            Err(e) => Ended::Code(io.fail(json, e.kind(), &e.to_string())),
        };
    }
    let command = cli.command.unwrap_or(Command::List);
    let keychain = (ctx.platform == Platform::MacOs).then(|| ctx.keychain.clone());
    let keychain_counts = match ctx.platform {
        Platform::MacOs => daemon_cmd::keychain_counts(
            std::iter::once(&ctx.keychain).chain(ctx.vault_keychain.as_ref()),
        ),
        Platform::Linux => Vec::new(),
    };
    let platform = ctx.platform;
    let color = ColorEnv::of(&ctx, cli.no_color);
    let provider_flag = cli.provider.map(ProviderId::new);
    let shown = Arc::clone(&ctx.shown_warnings);
    // §12.8: under a marker that cannot be read the outer home is unknown, and nothing runs.
    let (engine, warnings) = match engine_for(ctx, provider_flag.as_ref()) {
        Ok(built) => built,
        Err(e) => return Ended::Code(io.fail(json, e.kind(), &e.to_string())),
    };
    // Redesign §13: a warning the host already showed (the TUI's, at start-up) is not printed
    // again.
    for w in warnings.iter().filter(|w| !shown.contains(w)) {
        io.warn(w);
    }
    let mut app = App {
        engine,
        json,
        color,
        provider_flag,
        expect_account: cli.expect_account.map(AccountId::from_string),
        settings_warnings: warnings,
        keychain,
        platform,
        keychain_counts,
        io,
    };
    // `purge --provider P` also names a provider this build does not register, while the store
    // holds accounts of it (§10.5); `purge_plan` validates that, and refuses any other name as
    // unknown-provider all the same.
    if let Some(p) = app
        .provider_flag
        .as_ref()
        .filter(|_| !matches!(command, Command::Purge { .. }))
    {
        if let Err(e) = app.engine.provider(p) {
            return Ended::Code(app.io.fail(json, e.kind(), &e.to_string()));
        }
    }
    let result = match command {
        // §12.5: `run` has an end of its own. It checks the Keychain's lock itself, once it
        // knows it launches a session, and once `claude` has run, its code is the command's.
        Command::Run {
            account,
            require_session,
            args,
        } => app
            .run_agent(account, require_session, args)
            .map(Ended::Child),
        command => {
            // Only a command that touches a Keychain item checks its lock.
            let unlocked = if command.touches_keychain() {
                app.lock_check(&command)
            } else {
                Ok(())
            };
            unlocked
                .and_then(|()| app.dispatch(command))
                .map(Ended::Code)
        }
    };
    match result {
        Ok(ended) => ended,
        Err(failure) => failure.report(app.io, json, true),
    }
}

/// The command's name in its canonical spelling, for the late notice.
fn command_name(command: &Command) -> &'static str {
    match command {
        Command::List => "list",
        Command::Status => "status",
        Command::Tui { .. } => "tui",
        Command::Switch { .. } => "switch",
        Command::Add { .. } => "add",
        Command::AddToken { .. } => "add-token",
        Command::Remove { .. } => "remove",
        Command::Disable { .. } => "disable",
        Command::Enable { .. } => "enable",
        Command::Alias { .. } => "alias",
        Command::Move { .. } => "move",
        Command::History { .. } => "history",
        Command::Tokens { .. } => "tokens",
        Command::Map { .. } => "map",
        Command::Unmap { .. } => "unmap",
        Command::ShellInit { .. } => "shell-init",
        Command::Statusline { .. } => "statusline",
        Command::Auto { .. } => "auto",
        Command::Daemon { .. } => "daemon",
        Command::Run { .. } => "run",
        Command::Config { .. } => "config",
        Command::Displaced { .. } => "displaced",
        Command::Purge { .. } => "purge",
        Command::Export { .. } => "export",
        Command::Import { .. } => "import",
        Command::Doctor { .. } => "doctor",
        Command::Completions { .. } => "completions",
    }
}

impl App<'_, '_> {
    fn provider(&self) -> ProviderId {
        self.provider_flag
            .clone()
            .unwrap_or_else(|| self.engine.default_provider().clone())
    }

    /// A person can answer a prompt: never with `--json`, and only on a terminal.
    fn can_prompt(&self) -> bool {
        !self.json && self.io.prompter.interactive()
    }

    /// §14.1: a prompt is a cancellation point. One a signal cut short has
    /// answered as a decline, but whatever it answered, the command stops here, interrupted.
    fn after_prompt(&self) -> Result<(), Failure> {
        match self.engine.cancel().requested() {
            Some(signal) => Err(EngineError::Interrupted(signal).into()),
            None => Ok(()),
        }
    }

    fn print(&mut self, human: &str, json: Value) {
        if self.json {
            let _ = writeln!(self.io.out, "{json}");
        } else {
            let _ = write!(self.io.out, "{human}");
        }
    }

    /// Now, in epoch seconds, by the engine's clock: what countdowns and ages count from.
    fn now_s(&self) -> i64 {
        self.engine.now_ms().div_euclid(1000)
    }

    /// Whether `list` and `status` colour their percentages (§13.1).
    fn color(&self) -> bool {
        self.color.enabled(self.engine.settings().color)
    }

    /// Appendix A.3's check, for a command that will touch a Keychain item. With no store,
    /// `switch` and `remove` have no account to touch (§5), so they run none: a fresh machine
    /// gets their no-op or "no account matches" even with a locked keychain. A store created
    /// by another process meanwhile is covered too: the tri-state reads refuse on their own.
    /// `switch`'s offer to add an unmanaged login checks before adding (`switch`).
    fn lock_check(&mut self, command: &Command) -> Result<(), Failure> {
        // §10.5: a usage error, then the run shell's refusal, come before the Keychain is asked.
        if let Command::Purge {
            keychain_orphans, ..
        } = command
        {
            if *keychain_orphans && self.provider_flag.is_some() {
                return Err(Failure::Usage(KEYCHAIN_ORPHANS_WITH_PROVIDER.into()));
            }
            self.refuse_in_run_shell()?;
        }
        if command.touches_keychain_only_with_a_store() && self.engine.existing_store()?.is_none() {
            return Ok(());
        }
        self.ensure_unlocked()
    }

    /// §12.8: the engine's refusal inside a run shell, shared by every command that changes
    /// accounts or the live login.
    fn refuse_in_run_shell(&self) -> Result<(), Failure> {
        Ok(self.engine.refuse_inside_run_shell()?)
    }

    /// Appendix A.3, before the command touches anything. On a terminal, a locked keychain is
    /// offered for unlocking and macOS asks for the password itself: tagteam never sees it.
    /// Anywhere else the command fails at once, naming the unlock command. Linux has no check.
    fn ensure_unlocked(&mut self) -> Result<(), Failure> {
        let Some(keychain) = self.keychain.clone() else {
            return Ok(());
        };
        // `Unknown` proceeds: the tri-state reads refuse safely on their own.
        if keychain.lock_state() != LockState::Locked {
            return Ok(());
        }
        if self.can_prompt() {
            let yes = self.io.prompter.confirm_unlock();
            self.after_prompt()?;
            // macOS asks for the password on the terminal: a Ctrl-C there ends `security`,
            // which shares tagteam's process group (§14.1), and the command with it.
            if yes {
                let unlocked = keychain.unlock();
                self.after_prompt()?;
                if unlocked {
                    return Ok(());
                }
            }
        }
        Err(Failure::Message(
            KIND_KEYCHAIN_LOCKED,
            KEYCHAIN_LOCKED.into(),
        ))
    }

    /// This platform's service manager (D9): launchd on macOS, systemd on Linux, by
    /// `Context::platform`, which a test-support build can override.
    fn service(&self) -> ServiceKind {
        ServiceKind::for_platform(self.platform == Platform::MacOs)
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, command_name};

    #[test]
    fn the_late_notice_names_each_command_as_it_is_typed() {
        use clap::Parser;
        let cases: &[&[&str]] = &[
            &["list"],
            &["ls"],
            &["status"],
            &["switch"],
            &["add"],
            &["add-token", "x"],
            &["remove", "1"],
            &["rm", "1"],
            &["disable", "1"],
            &["enable", "1"],
            &["alias"],
            &["move", "1", "2"],
            &["history"],
            &["tokens"],
            &["tokens", "month", "1", "--by", "model"],
            &["tokens", "2", "--json"],
            &["statusline"],
            &["auto"],
            &["map"],
            &["unmap"],
            &["shell-init", "zsh"],
            &["run"],
            &["run", "2", "--", "--json"],
            &["config", "list"],
            &["displaced"],
            &["completions", "bash"],
            &["purge", "--yes"],
            &["export", "--plaintext"],
            &["import", "backup.age"],
            &["doctor"],
            &["daemon"],
        ];
        for args in cases {
            let cli = Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied()))
                .unwrap();
            let expected = match args[0] {
                "ls" => "list",
                "rm" => "remove",
                typed => typed,
            };
            assert_eq!(
                command_name(cli.command.as_ref().unwrap()),
                expected,
                "{args:?}"
            );
        }
    }
}
