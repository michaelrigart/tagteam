//! `tagteam tui`, and a bare `tagteam` on a terminal (TUI spec §3): the refusals, then the TUI
//! over engines built as `app::run_command` builds one, with the CLI's own command path behind
//! its actions (`tui_host`).

use std::io::{self, IsTerminal, Write};
use std::sync::Arc;

use tagteam_core::ProviderId;
use tagteam_engine::EngineError;
use tagteam_provider::Cancel;
use tagteam_provider::profile::RunShell;
use tagteam_tui::source::{EngineSource, Source, SourceFactory};
use tagteam_tui::{Screen, TuiOptions};

use crate::app::{self, Context};
use crate::cli::{Cli, Command, ScreenArg};
use crate::signals;
use crate::tui_host::CliHost;

/// §3: the TUI has no JSON form.
pub(crate) const TUI_UNDER_JSON: &str = "tui has no JSON output; run it without --json";
/// §3: it draws on a terminal, and reads its keys from one.
pub(crate) const NEEDS_TERMINAL: &str = "tagteam tui needs a terminal";

/// Runs the TUI, and returns the process's exit code (§3, §10.2).
pub(crate) fn run(cli: &Cli, ctx: Context) -> i32 {
    let (mut out, mut err) = (io::stdout(), io::stderr());
    // §3: the refusals come before the terminal is touched.
    if cli.json {
        let _ = writeln!(out, "{}", app::error_json(app::KIND_USAGE, TUI_UNDER_JSON));
        return app::EXIT_USAGE;
    }
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        let _ = writeln!(err, "tagteam: {NEEDS_TERMINAL}");
        return app::EXIT_USAGE;
    }
    // Core §12.8: an unreadable marker refuses, as it does every command but `statusline`.
    let registry = app::build_registry(&ctx);
    let (run_shell, env) = app::locate(ctx.env.clone(), &registry);
    if let RunShell::Unreadable { marker, detail } = &run_shell {
        let e = EngineError::RunShellUnreadable {
            marker: marker.clone(),
            detail: detail.clone(),
        };
        let _ = writeln!(err, "tagteam: {e}");
        return app::EXIT_ERROR;
    }
    let provider = cli.provider.clone().map(ProviderId::new);
    let shutdown = Cancel::new();
    // §2, §4.3: the lanes' engines take the shutdown token, never the signal token.
    let mut inside = ctx.clone();
    inside.env.cancel = shutdown.clone();
    // An engine of the UI thread's own, built as `run_command` builds one and dropped before
    // the TUI starts. It refuses an unknown `--provider`, prints the settings warnings once,
    // before the alternate screen hides them, and reads `ui.color`. It reads the settings file
    // and nothing else.
    let (probe, warnings) =
        app::build_engine(inside.clone(), registry, run_shell, env, provider.as_ref());
    for w in &warnings {
        let _ = writeln!(err, "warning: {w}");
    }
    if let Some(p) = &provider {
        if let Err(e) = probe.provider(p) {
            let _ = writeln!(err, "tagteam: {e}");
            return app::EXIT_ERROR;
        }
    }
    let color = app::color_enabled(
        cli.no_color,
        ctx.no_color_env,
        ctx.force_color_env,
        probe.settings().color,
        true,
    );
    // Spec §5.3: Tokens reports on `--provider`'s provider, else the default, in the system
    // zone, which is read here so that the UI thread reads no zone file (B6).
    let tokens_provider = provider
        .clone()
        .unwrap_or_else(|| probe.default_provider().clone());
    drop(probe);
    let tz = jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC);
    // §7.3: Ctrl-\ belongs to a foreground session, and the TUI survives it for its whole life.
    if let Err(e) = signals::survive_quit() {
        let _ = writeln!(
            err,
            "warning: Ctrl-\\ may stop tagteam while a session runs: {e}"
        );
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            let _ = writeln!(
                err,
                "tagteam: cannot find its own executable, which runs sessions: {e}"
            );
            return app::EXIT_ERROR;
        }
    };
    let host = Arc::new(CliHost::new(&ctx, shutdown.clone(), exe).with_startup_warnings(warnings));
    let screen = screen_for(cli);
    let opts = TuiOptions {
        screen,
        color,
        provider: provider.clone(),
        tokens_provider,
        tz,
        signal: ctx.env.cancel.clone(),
        shutdown,
    };
    let exit = tagteam_tui::run(opts, sources(inside, provider), host);
    if let Some(report) = &exit.report {
        let _ = write!(err, "{report}");
        if !report.ends_with('\n') {
            let _ = writeln!(err);
        }
    }
    exit.code
}

/// R4: each lane builds its source inside its own thread, from an engine built as
/// `run_command` builds one. `ctx`'s token is the shutdown token (§4.3). The settings warnings
/// were printed once already.
fn sources(ctx: Context, provider: Option<ProviderId>) -> SourceFactory {
    Arc::new(move || {
        let ctx = ctx.clone();
        let registry = app::build_registry(&ctx);
        let (run_shell, env) = app::locate(ctx.env.clone(), &registry);
        let (engine, _warnings) =
            app::build_engine(ctx, registry, run_shell, env, provider.as_ref());
        Box::new(EngineSource::new(engine)) as Box<dyn Source>
    })
}

/// The screen `tui` opens on: the one it names, else Accounts. Every `ScreenArg` is matched, so
/// a new one cannot open Accounts unnoticed.
fn screen_for(cli: &Cli) -> Screen {
    match &cli.command {
        Some(Command::Tui {
            screen: Some(arg), ..
        }) => match arg {
            ScreenArg::Accounts => Screen::Accounts,
            ScreenArg::History => Screen::History,
        },
        _ => Screen::Accounts,
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn tui_opens_on_the_screen_it_names_and_on_accounts_otherwise() {
        for (args, screen) in [
            (&["tagteam", "tui", "history"][..], Screen::History),
            (&["tagteam", "tui", "accounts"], Screen::Accounts),
            (&["tagteam", "tui"], Screen::Accounts),
            (&["tagteam"], Screen::Accounts),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert_eq!(screen_for(&cli), screen, "{args:?}");
        }
    }
}
