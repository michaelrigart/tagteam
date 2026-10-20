use serde_json::{Value, json};
use tagteam_core::{AccountId, Pace, ProviderId, Window};
use tagteam_engine::collect::CollectMode;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::{SwitchReason, SwitchRequest, SwitchTarget, UsageStrategy};
use tagteam_engine::views::{AccountView, ShellAccount, StatusView};
use tagteam_engine::{Engine, EngineError};
use tagteam_provider::profile::RunShell;

use crate::SCHEMA_VERSION;
use crate::cli::StrategyArg;
use crate::render;

use crate::app::App;
use crate::app::failure::{Failure, KIND_STALE_SELECTION, KIND_UNMANAGED_ACCOUNT, cancelled};

const STALE_SELECTION: &str = "the selected account changed; nothing was done";

const ALIAS_USAGE: &str = "alias takes ACCOUNT NAME, ACCOUNT --unset, or no arguments";

/// `active` for a change that has already committed: if it cannot be read, the account is
/// reported inactive and the failure is logged (by position and ID, never by email) rather
/// than failing a command that succeeded.
fn or_inactive(active: Result<bool, EngineError>, position: u32, id: &AccountId) -> bool {
    active.unwrap_or_else(|e| {
        tracing::warn!(
            target: "tagteam::app",
            position,
            account = %id,
            kind = e.kind(),
            "could not tell whether the account is active; reporting it inactive"
        );
        false
    })
}

/// `Provider::render_usage` for a row's provider (§13.2); null for a provider this build does
/// not register, whose rows no view lists.
fn render_usage(engine: &Engine) -> impl Fn(&ProviderId, &[(Window, Pace)]) -> Value + '_ {
    move |provider, windows| {
        engine
            .provider(provider)
            .map_or(Value::Null, |p| p.render_usage(windows))
    }
}

/// `--strategy`'s value as the engine names it (§9.3).
fn usage_strategy(s: StrategyArg) -> UsageStrategy {
    match s {
        StrategyArg::Best => UsageStrategy::Best,
        StrategyArg::NextAvailable => UsageStrategy::NextAvailable,
    }
}

/// `--model` (§9.3): a comma-separated list of model names, each trimmed, empty ones dropped.
/// `all` passes as written; §8.2's relevance matches it in any case. An empty list is still an
/// override: no model window counts for this switch.
pub(super) fn model_list(arg: &str) -> Vec<String> {
    arg.split(',')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
        .collect()
}

impl App<'_, '_> {
    /// The run shell's own account id, as its marker names it (§13.1's `▶ this`).
    fn this_account(&self) -> Option<AccountId> {
        match self.engine.run_shell() {
            RunShell::Inside { marker, .. } => Some(marker.account_id.clone()),
            RunShell::Outside | RunShell::Unreadable { .. } => None,
        }
    }

    /// §12.8: the run shell's account, for a `status` of the provider whose run shell this is.
    /// Another provider's status has no session to name.
    fn session_account(&self, provider: &ProviderId) -> Result<ShellAccount, EngineError> {
        match self.engine.run_shell() {
            RunShell::Inside { marker, .. } if &marker.provider == provider => {
                self.engine.shell_account()
            }
            _ => Ok(ShellAccount::NotInShell),
        }
    }

    /// The login the run shell's profile holds, as its provider reads it in the profile's
    /// directory, where the marker was found (Decision 19): its email, else its label. Only
    /// `status`'s text asks, for an account tagteam does not manage; §13.2's JSON has
    /// `session: null`, and `statusline` never parses a profile (§13.5).
    fn session_login(&self) -> Option<String> {
        let RunShell::Inside { profile, marker } = self.engine.run_shell() else {
            return None;
        };
        let p = self.engine.provider(&marker.provider).ok()?;
        p.profile_identity(self.engine.env(), profile)
            .present()
            .map(|i| i.email.unwrap_or(i.label))
    }

    fn notices(&mut self, notices: &[String]) {
        for n in notices {
            self.io.note(n);
        }
    }

    /// §8.3's on-demand collection, which the command waits for. A usage failure is never a
    /// command error: it shows in the account's row. The collector's warnings (an account
    /// whose own collection errored is one of them), and its error should collecting fail
    /// before any account starts (the store cannot be opened), go to stderr. With nothing to
    /// collect nothing is opened, so `list` on a fresh machine still creates nothing (§5).
    /// An interrupted collection is the command's interruption (§14.1), never a warning.
    fn collect(&mut self, accounts: Vec<AccountId>) -> Result<(), Failure> {
        if accounts.is_empty() {
            return Ok(());
        }
        let warnings = match self
            .engine
            .collect_usage(CollectMode::OnDemand { accounts })
        {
            Ok(report) => report.warnings,
            Err(e) if e.signal().is_some() => return Err(e.into()),
            Err(e) => vec![format!("usage was not collected: {e}")],
        };
        for w in warnings {
            self.io.warn(&w);
        }
        Ok(())
    }

    /// §10.4, with a terminal prompt for an ambiguous email. With `--expect-account`, the
    /// account it resolves to must be that one: another is refused as `stale-selection`, before
    /// the command has changed anything.
    pub(in crate::app) fn resolve(&mut self, input: &str) -> Result<AccountRow, Failure> {
        let row = match self.engine.resolve(input, self.provider_flag.as_ref()) {
            Err(EngineError::Ambiguous { .. }) if self.can_prompt() => {
                let found = self.engine.candidates(input, self.provider_flag.as_ref())?;
                let labels: Vec<String> = found
                    .iter()
                    .map(|r| format!("{} #{} {}", r.provider, r.position, render::name(r)))
                    .collect();
                let choice = self.io.prompter.choose("Which account?", &labels);
                self.after_prompt()?;
                choice
                    .and_then(|i| found.get(i).cloned())
                    .ok_or_else(cancelled)?
            }
            other => other?,
        };
        match &self.expect_account {
            Some(expected) if *expected != row.id => Err(Failure::Message(
                KIND_STALE_SELECTION,
                STALE_SELECTION.into(),
            )),
            _ => Ok(row),
        }
    }

    pub(in crate::app) fn list(&mut self) -> Result<(), Failure> {
        // §8.3: every listed account is offered to the collector, which fetches only
        // those that are due; the views are then read with whatever it recorded.
        let listed = self.engine.accounts(self.provider_flag.as_ref())?;
        let ids: Vec<AccountId> = listed
            .iter()
            .flat_map(|l| l.accounts.iter().map(|v| v.row.id.clone()))
            .collect();
        self.collect(ids)?;
        let lists = self.engine.accounts(self.provider_flag.as_ref())?;
        let this = self.this_account();
        let (now_s, color) = (self.now_s(), self.color());
        let engine = &self.engine;
        let names = |id: &str| {
            engine
                .provider(&ProviderId::new(id))
                .map_or_else(|_| id.to_owned(), |p| p.display_name().to_owned())
        };
        let launch = engine.provider(&self.provider())?.launch_command();
        let human = render::list_human(&lists, this.as_ref(), &names, launch, now_s, color);
        let json = render::list_json(&lists, &self.provider(), &render_usage(engine));
        self.print(&human, json);
        Ok(())
    }

    pub(in crate::app) fn status(&mut self) -> Result<(), Failure> {
        let provider = self.provider();
        // §8.3: `status` collects the live account only.
        if let StatusView::Managed { account, .. } = self.engine.status(&provider)? {
            self.collect(vec![account.row.id])?;
        }
        let s = self.engine.status(&provider)?;
        let session = self.session_account(&provider)?;
        let login = match session {
            ShellAccount::Unmanaged => self.session_login(),
            ShellAccount::NotInShell | ShellAccount::Managed(_) => None,
        };
        let (now_s, color) = (self.now_s(), self.color());
        let human = render::status_human(&s, &session, login.as_deref(), now_s, color);
        let json =
            render::status_json(&s, &session, provider.as_str(), &render_usage(&self.engine));
        self.print(&human, json);
        Ok(())
    }

    pub(in crate::app) fn add_token(
        &mut self,
        token: Option<String>,
        position: Option<u32>,
        email: Option<String>,
        alias: Option<String>,
        yes: bool,
    ) -> Result<(), Failure> {
        let token = self.token(token)?;
        let provider = self.provider();
        let out = self.confirming(yes, |engine, yes| {
            engine.add_token(AddTokenOptions {
                provider: provider.clone(),
                token: token.clone(),
                position,
                email: email.clone(),
                alias: alias.clone(),
                yes,
            })
        })?;
        self.added(out.account, out.created);
        Ok(())
    }

    pub(in crate::app) fn remove(&mut self, account: &str) -> Result<(), Failure> {
        let row = self.resolve(account)?;
        // Decided before the row is gone. Removal never touches the live login
        // (§10.3), so this also says whether the removed login is still the live one.
        let active = self.is_active(&row)?;
        let row = self.engine.remove(&row.id)?;
        let human = format!(
            "Removed {} (position {}).\n",
            render::name(&row),
            row.position
        );
        let view = self.engine.account_view(row, active);
        self.print_view(&human, view, None);
        Ok(())
    }

    pub(in crate::app) fn alias(
        &mut self,
        account: Option<String>,
        name: Option<String>,
        unset: bool,
    ) -> Result<(), Failure> {
        match (account, name, unset) {
            (None, None, false) => {
                let lists = self.engine.accounts(self.provider_flag.as_ref())?;
                let rows: Vec<&AccountRow> = lists
                    .iter()
                    .flat_map(|l| l.accounts.iter().map(|v| &v.row))
                    .filter(|r| r.alias.is_some())
                    .collect();
                let human: String = rows
                    .iter()
                    .map(|r| {
                        format!(
                            "{}  {}  {}\n",
                            r.alias.as_deref().unwrap_or_default(),
                            r.position,
                            render::email(r)
                        )
                    })
                    .collect();
                let aliases: Vec<Value> = rows
                    .iter()
                    .map(|r| json!({"alias": r.alias, "number": r.position, "provider": r.provider.as_str()}))
                    .collect();
                self.print(
                    &human,
                    json!({"schemaVersion": SCHEMA_VERSION, "aliases": aliases}),
                );
            }
            (Some(account), Some(name), false) => {
                let row = self.resolve(&account)?;
                let row = self.engine.set_alias(&row.id, Some(&name))?;
                let alias = row.alias.clone().unwrap_or_default();
                let human = format!("Position {} is now {alias}.\n", row.position);
                self.print_account(&human, row, None);
            }
            (Some(account), None, true) => {
                let row = self.resolve(&account)?;
                let row = self.engine.set_alias(&row.id, None)?;
                let human = format!("Position {} has no alias now.\n", row.position);
                self.print_account(&human, row, None);
            }
            _ => return Err(Failure::Usage(ALIAS_USAGE.into())),
        }
        Ok(())
    }

    pub(in crate::app) fn move_account(
        &mut self,
        account: &str,
        position: u32,
    ) -> Result<(), Failure> {
        let row = self.resolve(account)?;
        let row = self.engine.move_to(&row.id, position)?;
        let human = format!(
            "{} is now at position {}.\n",
            render::email(&row),
            row.position
        );
        self.print_account(&human, row, None);
        Ok(())
    }

    /// Whether `row` is its provider's active account, decided as `list` decides it: by the
    /// engine's views (the live login; the store's active account while the live identity is
    /// unreadable).
    fn is_active(&self, row: &AccountRow) -> Result<bool, EngineError> {
        Ok(self
            .engine
            .accounts(Some(&row.provider))?
            .iter()
            .flat_map(|l| &l.accounts)
            .any(|v| v.active && v.row.id == row.id))
    }

    fn print_view(&mut self, human: &str, view: AccountView, created: Option<bool>) {
        let json = render::account_json(&view, created, &render_usage(&self.engine));
        self.print(human, json);
    }

    /// An account command's result, with `active` as the engine sees it after the command.
    /// The change has committed by now, so a failure to read `active` never fails it.
    pub(in crate::app) fn print_account(
        &mut self,
        human: &str,
        row: AccountRow,
        created: Option<bool>,
    ) {
        let active = or_inactive(self.is_active(&row), row.position, &row.id);
        let view = self.engine.account_view(row, active);
        self.print_view(human, view, created);
    }

    fn added(&mut self, account: AccountRow, created: bool) {
        let verb = if created { "Added" } else { "Updated" };
        let human = format!(
            "{verb} {} at position {}.\n",
            render::name(&account),
            account.position
        );
        self.print_account(&human, account, Some(created));
    }

    /// §10.1: `--position` over another account needs confirmation, or `--yes`. A person is
    /// asked, and a yes runs `write` again with `yes` set, with whatever it has already read;
    /// anywhere else the engine's refusal, which names `--yes`, stands.
    fn confirming<T>(
        &mut self,
        yes: bool,
        write: impl Fn(&Engine, bool) -> Result<T, EngineError>,
    ) -> Result<T, Failure> {
        match write(&self.engine, yes) {
            Err(EngineError::NeedsConfirmation { position, occupant }) if self.can_prompt() => {
                let question = format!("Position {position} holds {occupant}. Replace it?");
                let yes = self.io.prompter.confirm(&question, false);
                self.after_prompt()?;
                if !yes {
                    return Err(cancelled());
                }
                Ok(write(&self.engine, true)?)
            }
            other => Ok(other?),
        }
    }

    pub(in crate::app) fn add(
        &mut self,
        provider: ProviderId,
        position: Option<u32>,
        alias: Option<String>,
        yes: bool,
    ) -> Result<(), Failure> {
        let out = self.confirming(yes, |engine, yes| {
            engine.add_live(AddOptions {
                provider: provider.clone(),
                position,
                alias: alias.clone(),
                yes,
            })
        })?;
        self.notices(&out.notices);
        self.added(out.account, out.created);
        Ok(())
    }

    /// §9: a direct switch to ACCOUNT, a bare rotation, or a usage strategy (§9.3) with its
    /// `--model` override. clap has already refused `--strategy` with an ACCOUNT and `--model`
    /// without `--strategy` (Decision 7).
    pub(in crate::app) fn switch(
        &mut self,
        account: Option<String>,
        force: bool,
        strategy: Option<StrategyArg>,
        model: Option<String>,
    ) -> Result<(), Failure> {
        let (target, provider) = match (&account, strategy) {
            (Some(a), _) => {
                let row = self.resolve(a)?;
                (SwitchTarget::Account(row.id), row.provider)
            }
            (None, Some(s)) => (
                SwitchTarget::Usage {
                    strategy: usage_strategy(s),
                    models: model.as_deref().map(model_list),
                },
                self.provider(),
            ),
            (None, None) => (SwitchTarget::Rotation, self.provider()),
        };
        let req = || SwitchRequest {
            provider: provider.clone(),
            target: target.clone(),
            force,
            source: "cli",
            auto: None,
        };
        let mut outcome = self.engine.switch(req())?;
        // §9.2: `--json` reports the no-op as is; a terminal offers to add the login first.
        if outcome.reason == SwitchReason::UnmanagedAccount && !self.json {
            let email = outcome.unmanaged_email.clone().unwrap_or_default();
            if !self.io.prompter.interactive() {
                return Err(Failure::Message(
                    KIND_UNMANAGED_ACCOUNT,
                    format!(
                        "the live login ({email}) is not managed by tagteam; run `tagteam add` first, or pass --force"
                    ),
                ));
            }
            let add = self
                .io
                .prompter
                .confirm(&format!("Add the current login ({email}) first?"), true);
            self.after_prompt()?;
            if !add {
                return Err(cancelled());
            }
            // With no store, `lock_check` ran no check for this `switch`, and adding reads the
            // live credential and writes the vault: Appendix A.3 applies before it does.
            self.ensure_unlocked()?;
            self.add(provider.clone(), None, None, false)?;
            outcome = self.engine.switch(req())?;
        }
        for w in &outcome.warnings {
            self.io.warn(w);
        }
        // As the engine reports it for this write: a file that is the platform's only store is
        // routine and says nothing.
        if let Some(notice) = render::fallback_notice(&outcome) {
            self.io.warn(&notice);
        }
        self.print(
            &render::switch_human(&outcome),
            render::switch_json(&outcome, provider.as_str()),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AccountId, EngineError, StrategyArg, UsageStrategy, model_list, or_inactive, usage_strategy,
    };

    #[test]
    fn an_unreadable_active_flag_after_a_commit_is_inactive_not_an_error() {
        let id = AccountId::from_string("0192");
        let failed = Err(EngineError::Io(std::io::Error::other("store went away")));
        assert!(!or_inactive(failed, 1, &id));
        assert!(or_inactive(Ok(true), 1, &id));
    }

    #[test]
    fn model_lists_are_trimmed_and_drop_empty_names() {
        assert_eq!(model_list("Fable"), ["Fable"]);
        assert_eq!(
            model_list(" Fable , opus ,, all "),
            ["Fable", "opus", "all"]
        );
        assert!(model_list("").is_empty());
        assert!(model_list(" , ").is_empty());
    }

    #[test]
    fn each_strategy_flag_names_its_engine_strategy() {
        assert_eq!(usage_strategy(StrategyArg::Best), UsageStrategy::Best);
        assert_eq!(
            usage_strategy(StrategyArg::NextAvailable),
            UsageStrategy::NextAvailable
        );
    }
}
