use tagteam_core::ProviderId;
use tagteam_engine::EngineError;
use tagteam_engine::store::AccountRow;
use tagteam_engine::tokens::{AccountSel, TokenQuery};
use tagteam_engine::views::StatusView;

use crate::cli::{TokensArgs, TokensPeriod};
use crate::{history, tokens_cmd};

use crate::app::App;
use crate::app::failure::{Failure, KIND_NO_LIVE_LOGIN, KIND_UNMANAGED_ACCOUNT, KIND_UNSUPPORTED};

const CSV_AND_JSON: &str = "--csv and --json are two output formats; pass one";
const BAD_SINCE: &str = "--since takes a span like 14d, 12h or 30m";

/// The refusal for a command that needs a live login, naming the provider's `launch` command.
fn no_live_login(launch: &str) -> String {
    format!("there is no live login; name an account, or log in with `{launch}` first")
}

impl App<'_, '_> {
    /// §13.4: reads `usage_samples` only, so it never fetches. ACCOUNT defaults to the live
    /// login's account; without `--window`, only the windows that count for switching show.
    pub(in crate::app) fn history(
        &mut self,
        account: Option<String>,
        window: Option<String>,
        since: &str,
        csv: bool,
    ) -> Result<(), Failure> {
        if csv && self.json {
            return Err(Failure::Usage(CSV_AND_JSON.into()));
        }
        let span = history::parse_since(since).ok_or_else(|| Failure::Usage(BAD_SINCE.into()))?;
        let row = match &account {
            Some(a) => self.resolve(a)?,
            None => self.live_row()?,
        };
        let now_s = self.now_s();
        // The engine's relevance filter is the one rule for which windows show (§8.2).
        let view = self
            .engine
            .history(&row.id, window.as_deref(), now_s.saturating_sub(span))?;
        if csv {
            let _ = write!(self.io.out, "{}", history::csv(&view));
        } else {
            self.print(
                &history::human(&view, window.as_deref(), since, now_s),
                history::json(&view, row.provider.as_str()),
            );
        }
        Ok(())
    }

    /// Token report spec §11: `tokens`. Every flag is checked before anything is read; the
    /// accounts named choose the provider unless `--provider` does (core §13.1); the index is
    /// refreshed (unless `--no-refresh`), and `--model` is then resolved against it. Like any
    /// writing command it creates the store. It never fetches and touches no Keychain item.
    pub(in crate::app) fn tokens(
        &mut self,
        period: Option<Box<TokensPeriod>>,
        args: TokensArgs,
    ) -> Result<(), Failure> {
        let (period, args) = TokensPeriod::effective(period, args);
        if args.csv && self.json {
            return Err(Failure::Usage(CSV_AND_JSON.into()));
        }
        let flags = tokens_cmd::parse_flags(&args, &period, self.engine.now_ms())
            .map_err(Failure::Usage)?;
        // Core §10.4 scopes each ACCOUNT to --provider when it is given, as for any command; the
        // accounts named then choose the provider unless --provider does (core §13.1).
        let mut named = Vec::new();
        for input in &args.accounts {
            named.push(self.resolve(input)?);
        }
        let positions: Vec<(u32, ProviderId)> = named
            .iter()
            .map(|row| (row.position, row.provider.clone()))
            .collect();
        let provider = tokens_cmd::provider_for(
            self.provider_flag.as_ref(),
            self.engine.default_provider(),
            &positions,
        )
        .map_err(Failure::Usage)?;
        let p = self.engine.provider(&provider)?;
        if !p.capabilities().transcripts {
            return Err(Failure::Message(
                KIND_UNSUPPORTED,
                format!("{} has no transcripts", p.display_name()),
            ));
        }
        let app = p.display_name().to_owned();
        let mut accounts: Vec<AccountSel> = named
            .into_iter()
            .map(|row| AccountSel::Account(row.id))
            .collect();
        if args.unattributed {
            accounts.push(AccountSel::Unattributed);
        }
        let store = self.engine.store()?;
        if !args.no_refresh {
            tokens_cmd::refresh(&self.engine, &provider, &mut *self.io.err)?;
        }
        if let Some(signal) = self.engine.cancel().requested() {
            return Err(EngineError::Interrupted(signal).into());
        }
        let models = if args.model.is_empty() {
            None
        } else {
            let indexed = store.token_models(&provider).map_err(EngineError::from)?;
            Some(tokens_cmd::resolve_models(&args.model, &indexed).map_err(Failure::Usage)?)
        };
        let report = self.engine.token_report(&TokenQuery {
            provider,
            period,
            grouping: flags.grouping,
            since_ms: Some(flags.since_ms),
            until_ms: flags.until_ms,
            accounts: (!accounts.is_empty()).then_some(accounts),
            models,
            session: None,
            tz: flags.tz,
        })?;
        for notice in tokens_cmd::notices(&report, args.no_refresh, &app) {
            let _ = writeln!(self.io.err, "{notice}");
        }
        if args.csv {
            let _ = write!(self.io.out, "{}", tokens_cmd::csv(&report));
        } else {
            self.print(&tokens_cmd::human(&report), tokens_cmd::json(&report));
        }
        Ok(())
    }

    /// The live login's account, for a command whose ACCOUNT defaults to it.
    fn live_row(&self) -> Result<AccountRow, Failure> {
        match self.engine.status(&self.provider())? {
            StatusView::Managed { account, .. } => Ok(account.row),
            StatusView::Unmanaged { email } => Err(Failure::Message(
                KIND_UNMANAGED_ACCOUNT,
                format!(
                    "the live login ({email}) is not managed by tagteam; name an account, or run `tagteam add` first"
                ),
            )),
            StatusView::NoLogin => {
                let launch = self.engine.provider(&self.provider())?.launch_command();
                Err(Failure::Message(KIND_NO_LIVE_LOGIN, no_live_login(launch)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::no_live_login;

    #[test]
    fn no_live_login_names_the_launch_command() {
        assert_eq!(
            no_live_login("claude"),
            "there is no live login; name an account, or log in with `claude` first"
        );
    }
}
