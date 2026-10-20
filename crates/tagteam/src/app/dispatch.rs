use crate::cli::Command;
use crate::render;

use super::App;
use super::commands::auto_flags;
use super::failure::Failure;

impl App<'_, '_> {
    /// Runs `command` and returns its exit code: 0, except for `auto` (§11.4).
    pub(super) fn dispatch(&mut self, command: Command) -> Result<i32, Failure> {
        match command {
            Command::List => self.list()?,
            Command::Status => self.status()?,
            Command::Switch {
                account,
                force,
                strategy,
                model,
            } => self.switch(account, force, strategy, model)?,
            Command::Add {
                position,
                alias,
                yes,
            } => self.add(self.provider(), position, alias, yes)?,
            Command::AddToken {
                token,
                position,
                email,
                alias,
                yes,
            } => self.add_token(token, position, email, alias, yes)?,
            Command::Remove { account } => self.remove(&account)?,
            Command::Disable { account } => {
                let row = self.resolve(&account)?;
                let row = self.engine.set_disabled(&row.id, true)?;
                self.print_account(&format!("{} is disabled.\n", render::name(&row)), row, None);
            }
            Command::Enable { account } => {
                let row = self.resolve(&account)?;
                let row = self.engine.set_disabled(&row.id, false)?;
                self.print_account(&format!("{} is enabled.\n", render::name(&row)), row, None);
            }
            Command::Alias {
                account,
                name,
                unset,
            } => self.alias(account, name, unset)?,
            Command::Move { account, position } => self.move_account(&account, position)?,
            Command::History {
                account,
                window,
                since,
                csv,
            } => self.history(account, window, &since, csv)?,
            Command::Tokens { period, args } => self.tokens(period, args)?,
            Command::Map { account, path } => match account {
                None => self.list_mappings()?,
                Some(account) => self.map(&account, path)?,
            },
            Command::Unmap { path } => self.unmap(path)?,
            Command::Statusline { .. } => unreachable!("run answers statusline before dispatch"),
            Command::ShellInit { .. } => unreachable!("run answers shell-init before dispatch"),
            Command::Run { .. } => unreachable!("run_command answers run before dispatch"),
            Command::Tui { .. } => unreachable!("run_command answers tui before dispatch"),
            Command::Config { action } => self.config(action)?,
            Command::Displaced { purge, yes } => self.displaced(purge, yes)?,
            Command::Purge {
                yes,
                keychain_orphans,
            } => return self.purge(yes, keychain_orphans),
            Command::Export {
                file,
                account,
                full,
                recipient,
                recipient_file,
                plaintext,
            } => self.export(file, &account, full, &recipient, &recipient_file, plaintext)?,
            Command::Import {
                file,
                force,
                identity,
            } => return self.import(&file, force, &identity),
            // §13.6: read-only, so no Keychain check precedes it; it exits 1 when a check fails.
            Command::Doctor { online } => return self.doctor(online),
            Command::Completions { .. } => unreachable!("run answers completions before dispatch"),
            Command::Auto {
                once,
                dry_run,
                threshold,
                interval,
                cooldown,
                strategy,
                model,
                include_api_key_accounts,
            } => {
                let flags = auto_flags(
                    threshold,
                    interval,
                    cooldown,
                    strategy,
                    model,
                    include_api_key_accounts,
                )?;
                return self.auto_command(once, dry_run, flags);
            }
            Command::Daemon { action: None } => return self.daemon(),
            // D8, D9: the service commands. Root is refused for every command already.
            Command::Daemon {
                action: Some(action),
            } => return self.daemon_action(action),
        }
        Ok(0)
    }
}
