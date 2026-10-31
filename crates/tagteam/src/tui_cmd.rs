//! `tagteam tui`, and a bare `tagteam` on a terminal (TUI spec §3): the refusals, then the TUI
//! over engines built as `app::run_command` builds one, with the CLI's own command path behind
//! its actions (`tui_host`).

use std::io::{self, IsTerminal, Write};
use std::sync::Arc;

use tagteam_core::{AccountId, ProviderId};
use tagteam_engine::EventSource;
use tagteam_engine::daemon::daemon_socket_path;
use tagteam_engine::settings::Settings;
use tagteam_engine::timeline::{Scope, TimelineView};
use tagteam_engine::tokens::{IndexProgress, IndexState, RefreshOutcome, TokenQuery, TokenReport};
use tagteam_engine::views::ProviderNames;
use tagteam_provider::Cancel;
use tagteam_tui::source::{
    AccountsSnapshot, CollectSummary, EngineSource, Source, SourceError, SourceFactory,
};
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
    let provider = cli.provider.clone().map(ProviderId::new);
    let shutdown = Cancel::new();
    // §2, §4.3: the lanes' engines take the shutdown token, never the signal token.
    let mut inside = ctx.clone();
    inside.env.cancel = shutdown.clone();
    // `Settings::mtime` is read before the engine loads the settings, as it advises: a write
    // landing between the two then counts as one more change at the next accounts read. The
    // run shell's lookup changes no config directory, so the context's environment serves.
    let config_mtime = Settings::mtime(&inside.env);
    // An engine of the UI thread's own, built as `run_command` builds one and dropped before
    // the TUI starts. Core §12.8: an unreadable marker refuses, as it does every command but
    // `statusline`. It refuses an unknown `--provider`, prints the settings warnings once,
    // before the alternate screen hides them, and reads `ui.color`. It reads the settings file
    // and nothing else.
    let (probe, warnings) =
        match app::engine_for(inside.clone(), provider.as_ref(), EventSource::Cli) {
            Ok(built) => built,
            Err(e) => return app::fail_to(&mut out, &mut err, false, e.kind(), &e.to_string()),
        };
    for w in &warnings {
        let _ = writeln!(err, "warning: {w}");
    }
    if let Some(p) = &provider
        && let Err(e) = probe.provider(p)
    {
        return app::fail_to(&mut out, &mut err, false, e.kind(), &e.to_string());
    }
    // The terminal check above stands in for `stdout_terminal`: the TUI draws on stdout.
    let color = app::ColorEnv {
        stdout_terminal: true,
        ..app::ColorEnv::of(&ctx, cli.no_color)
    }
    .enabled(probe.settings().color);
    // Spec §5.3: Tokens reports on `--provider`'s provider, else the default, in the system
    // zone, which is read here so that the UI thread reads no zone file (B6).
    let tokens_provider = provider
        .clone()
        .unwrap_or_else(|| probe.default_provider().clone());
    // Redesign §12.2: the default provider's names, for a provider the accounts view does not
    // list.
    let names = match probe.provider(&tokens_provider) {
        Ok(p) => ProviderNames::of(&*p),
        Err(e) => return app::fail_to(&mut out, &mut err, false, e.kind(), &e.to_string()),
    };
    // Spec §8: the daemon of the store the TUI reads, found from the environment alone (D5.1).
    let daemon = daemon_socket_path(probe.env());
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
    let host = Arc::new(CliHost::new(&ctx, shutdown.clone(), exe, warnings.clone()));
    let screen = screen_for(cli);
    let opts = TuiOptions {
        screen,
        color,
        truecolor: matches!(
            std::env::var("COLORTERM").as_deref(),
            Ok("truecolor" | "24bit")
        ),
        provider: provider.clone(),
        tokens_provider,
        tz,
        signal: ctx.env.cancel.clone(),
        shutdown,
        daemon: Some(daemon),
        names,
        warnings,
        config_mtime,
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
/// were printed once already. A lane whose engine cannot be built refuses every read with why.
fn sources(ctx: Context, provider: Option<ProviderId>) -> SourceFactory {
    Arc::new(
        move || match app::engine_for(ctx.clone(), provider.as_ref(), EventSource::Cli) {
            Ok((engine, _warnings)) => Box::new(EngineSource::new(engine)) as Box<dyn Source>,
            Err(e) => Box::new(Refused(SourceError::from(e))),
        },
    )
}

/// The source of a lane whose engine could not be built: every read answers with that error,
/// in the CLI's own words.
struct Refused(SourceError);

impl Source for Refused {
    fn accounts(&self, _provider: Option<&ProviderId>) -> Result<AccountsSnapshot, SourceError> {
        Err(self.0.clone())
    }

    fn timeline(
        &self,
        _provider: &ProviderId,
        _scope: &Scope,
        _since_s: i64,
        _columns: usize,
    ) -> Result<TimelineView, SourceError> {
        Err(self.0.clone())
    }

    fn collect_due(&self, _accounts: &[AccountId]) -> Result<CollectSummary, SourceError> {
        Err(self.0.clone())
    }

    fn refresh_tokens(
        &self,
        _provider: &ProviderId,
        _progress: &mut dyn FnMut(&IndexProgress),
    ) -> Result<RefreshOutcome, SourceError> {
        Err(self.0.clone())
    }

    fn token_report(&self, _query: &TokenQuery) -> Result<TokenReport, SourceError> {
        Err(self.0.clone())
    }

    fn token_index_state(&self, _provider: &ProviderId) -> Result<IndexState, SourceError> {
        Err(self.0.clone())
    }
}

/// The screen `tui` opens on: the one it names, else Accounts. Every `ScreenArg` is matched, so
/// a new one cannot open Accounts unnoticed.
fn screen_for(cli: &Cli) -> Screen {
    match &cli.command {
        Some(Command::Tui {
            screen: Some(arg), ..
        }) => match arg {
            ScreenArg::Accounts => Screen::Accounts,
            ScreenArg::Tokens => Screen::Tokens,
            ScreenArg::History => Screen::History,
        },
        _ => Screen::Accounts,
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use tagteam_engine::EngineError;

    use super::*;

    #[test]
    fn tui_opens_on_the_screen_it_names_and_on_accounts_otherwise() {
        for (args, screen) in [
            (&["tagteam", "tui", "history"][..], Screen::History),
            (&["tagteam", "tui", "tokens"], Screen::Tokens),
            (&["tagteam", "tui", "accounts"], Screen::Accounts),
            (&["tagteam", "tui"], Screen::Accounts),
            (&["tagteam"], Screen::Accounts),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert_eq!(screen_for(&cli), screen, "{args:?}");
        }
    }

    #[test]
    fn a_source_whose_engine_cannot_be_built_refuses_every_read_in_the_cli_s_words() {
        use tagteam_engine::tokens::{TokenGrouping, TokenPeriod};

        let e = EngineError::RunShellUnreadable {
            marker: std::path::PathBuf::from("/run-shell/marker"),
            detail: "permission denied".into(),
        };
        let want = SourceError {
            kind: e.kind().to_owned(),
            message: e.to_string(),
        };
        let source = Refused(SourceError::from(e));
        let provider = ProviderId::new("fake-agent");
        let account = AccountId::from_string("a1");
        let query = TokenQuery {
            provider: provider.clone(),
            period: TokenPeriod::Day,
            grouping: TokenGrouping::Account,
            since_ms: None,
            until_ms: None,
            accounts: None,
            models: None,
            session: None,
            tz: jiff::tz::TimeZone::UTC,
        };
        assert_eq!(source.accounts(None).unwrap_err(), want, "accounts");
        assert_eq!(
            source.timeline(&provider, &Scope::All, 0, 70).unwrap_err(),
            want,
            "timeline"
        );
        assert_eq!(
            source
                .collect_due(std::slice::from_ref(&account))
                .unwrap_err(),
            want,
            "collect_due"
        );
        assert_eq!(
            source.refresh_tokens(&provider, &mut |_| {}).unwrap_err(),
            want,
            "refresh_tokens"
        );
        assert_eq!(
            source.token_report(&query).unwrap_err(),
            want,
            "token_report"
        );
        assert_eq!(
            source.token_index_state(&provider).unwrap_err(),
            want,
            "token_index_state"
        );
        assert_eq!(want.kind, "run-shell-unreadable");
    }
}
