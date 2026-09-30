use std::ffi::OsString;
use std::io::{BufRead, IsTerminal, Write};
use std::sync::Arc;

use serde_json::{Value, json};
use tagteam_cc::ClaudeCode;
use tagteam_cc::live::Platform;
use tagteam_core::{AccountId, CLAUDE_CODE, ProviderId};
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::net::UreqHttp;
use tagteam_engine::oracle::NoOracle;
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::{SwitchReason, SwitchRequest, SwitchTarget};
use tagteam_engine::vault::{FileVault, KeychainVault, Vault};
use tagteam_engine::views::AccountView;
use tagteam_engine::{Engine, EngineConfig, EngineError};
use tagteam_provider::security::SecurityCli;
use tagteam_provider::{Env, Keychain, LockState, SystemClock};

use crate::cli::{Cli, Command};
use crate::prompt::Prompter;
use crate::{render, root_guard};

/// §13.1.
pub(crate) const EXIT_ERROR: i32 = 1;
pub(crate) const EXIT_USAGE: i32 = 2;

/// `error.type` kinds the CLI raises itself; the engine's come from `EngineError::kind`.
pub(crate) const KIND_USAGE: &str = "usage";
const KIND_ROOT: &str = "root";
const KIND_KEYCHAIN_LOCKED: &str = "keychain-locked";
const KIND_CANCELLED: &str = "cancelled";
const KIND_INVALID_INPUT: &str = "invalid-input";
const KIND_UNMANAGED_ACCOUNT: &str = "unmanaged-account";

/// Appendix A.3. The default keychain is the login keychain, so the hint names its file.
const UNLOCK_QUESTION: &str = "The login keychain is locked (common over SSH). Unlock it now?";
const KEYCHAIN_LOCKED: &str = "the login keychain is locked (common over SSH); run `security unlock-keychain ~/Library/Keychains/login.keychain-db`, then retry";
const CANCELLED: &str = "cancelled";
const TOKEN_MISSING: &str = "pass the token as an argument, or `-` to read it from stdin";
const ALIAS_USAGE: &str = "alias takes ACCOUNT NAME, ACCOUNT --unset, or no arguments";

/// Honoured only with the `test-support` feature: a release build never reads them.
#[cfg(any(test, feature = "test-support"))]
const TEST_KEYCHAIN_DIR: &str = "TAGTEAM_TEST_KEYCHAIN_DIR";
#[cfg(any(test, feature = "test-support"))]
const TEST_PLATFORM: &str = "TAGTEAM_TEST_PLATFORM";

pub struct Context {
    pub env: Env,
    pub keychain: Arc<dyn Keychain>,
    pub platform: Platform,
}

type Overrides = (Option<Arc<dyn Keychain>>, Option<Platform>);

/// The test harness's keychain and platform, read through `var` so a test can supply the
/// environment without mutating the process's.
#[cfg(feature = "test-support")]
fn test_overrides(var: &dyn Fn(&str) -> Option<OsString>) -> Overrides {
    let kc = var(TEST_KEYCHAIN_DIR).map(|d| {
        Arc::new(tagteam_provider::FileKeychain::new(
            std::path::PathBuf::from(d),
        )) as Arc<dyn Keychain>
    });
    let platform = match var(TEST_PLATFORM).as_deref().and_then(|v| v.to_str()) {
        Some("linux") => Some(Platform::Linux),
        Some("macos") => Some(Platform::MacOs),
        _ => None,
    };
    (kc, platform)
}

/// A release build has no test overrides, whatever the environment holds.
#[cfg(not(feature = "test-support"))]
fn test_overrides(_var: &dyn Fn(&str) -> Option<OsString>) -> Overrides {
    (None, None)
}

impl Context {
    pub fn from_process() -> Self {
        let (kc, platform) = test_overrides(&|k| std::env::var_os(k));
        Self {
            env: Env::from_process(),
            keychain: kc.unwrap_or_else(|| Arc::new(SecurityCli::new())),
            platform: platform.unwrap_or_else(Platform::current),
        }
    }
}

pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub prompter: &'a mut dyn Prompter,
}

fn build_engine(ctx: Context) -> Engine {
    let cc = Arc::new(ClaudeCode::new(ctx.keychain.clone(), ctx.platform));
    let vault = match ctx.platform {
        Platform::MacOs => Vault::new(Box::new(KeychainVault::new(ctx.keychain))),
        Platform::Linux => Vault::new(Box::new(FileVault::new(ctx.env.data_dir().join("vault")))),
    };
    Engine::new(EngineConfig {
        env: ctx.env,
        registry: ProviderRegistry::new().with(cc),
        vault,
        oracle: Arc::new(NoOracle),
        clock: Arc::new(SystemClock),
        http: Arc::new(UreqHttp::new()),
        default_provider: ProviderId::new(CLAUDE_CODE),
    })
}

/// Logs are diagnostics, on stderr: ERROR by default, so a routine command stays quiet (what a
/// user must know reaches them as a notice instead), and DEBUG with `--debug`. Colour only on
/// a terminal.
fn init_logging(debug: bool, color: bool) {
    let level = if debug {
        tracing::Level::DEBUG
    } else {
        tracing::Level::ERROR
    };
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(color && std::io::stderr().is_terminal())
        .with_target(false)
        .with_max_level(level)
        .try_init();
}

enum Failure {
    Engine(EngineError),
    Usage(String),
    Message(&'static str, String),
}

impl From<EngineError> for Failure {
    fn from(e: EngineError) -> Self {
        Failure::Engine(e)
    }
}

fn cancelled() -> Failure {
    Failure::Message(KIND_CANCELLED, CANCELLED.into())
}

/// `active` for a change that has already committed: if it cannot be read, the account is
/// reported inactive and the failure is logged (by position and ID, never by email) rather
/// than failing a command that succeeded.
fn or_inactive(active: Result<bool, EngineError>, position: u32, id: &AccountId) -> bool {
    active.unwrap_or_else(|e| {
        tracing::warn!(
            position,
            id = %id,
            kind = e.kind(),
            "could not tell whether the account is active; reporting it inactive"
        );
        false
    })
}

/// §13.2: the one object `--json` prints for any error.
pub(crate) fn error_json(kind: &str, message: &str) -> Value {
    json!({"schemaVersion": 1, "error": {"type": kind, "message": message}})
}

struct App<'a, 'b> {
    engine: Engine,
    json: bool,
    provider_flag: Option<ProviderId>,
    /// The Keychain Appendix A.3's lock check asks: macOS only, since Linux has none.
    keychain: Option<Arc<dyn Keychain>>,
    io: &'a mut Io<'b>,
}

pub fn run(cli: Cli, ctx: Context, io: &mut Io<'_>) -> i32 {
    let color = !cli.no_color && std::env::var_os("NO_COLOR").is_none();
    init_logging(cli.debug, color);
    let json = cli.json;
    if let Err(msg) = root_guard::refuse_root() {
        return fail(io, json, KIND_ROOT, &msg);
    }
    let command = cli.command.unwrap_or(Command::List);
    let keychain = (ctx.platform == Platform::MacOs).then(|| ctx.keychain.clone());
    let mut app = App {
        engine: build_engine(ctx),
        json,
        provider_flag: cli.provider.map(ProviderId::new),
        keychain,
        io,
    };
    if let Some(p) = &app.provider_flag {
        if let Err(e) = app.engine.provider(p) {
            return fail(app.io, json, e.kind(), &e.to_string());
        }
    }
    // Only a command that touches a Keychain item checks its lock.
    let unlocked = if command.touches_keychain() {
        app.lock_check(&command)
    } else {
        Ok(())
    };
    let result = unlocked.and_then(|()| app.dispatch(command));
    match result {
        Ok(()) => 0,
        Err(Failure::Engine(e)) => fail(app.io, json, e.kind(), &e.to_string()),
        Err(Failure::Message(kind, m)) => fail(app.io, json, kind, &m),
        Err(Failure::Usage(m)) => {
            fail(app.io, json, KIND_USAGE, &m);
            EXIT_USAGE
        }
    }
}

fn fail(io: &mut Io<'_>, json: bool, kind: &str, message: &str) -> i32 {
    if json {
        let _ = writeln!(io.out, "{}", error_json(kind, message));
    } else {
        let _ = writeln!(io.err, "tagteam: {message}");
    }
    EXIT_ERROR
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

    fn print(&mut self, human: &str, json: Value) {
        if self.json {
            let _ = writeln!(self.io.out, "{json}");
        } else {
            let _ = write!(self.io.out, "{human}");
        }
    }

    fn notices(&mut self, notices: &[String]) {
        for n in notices {
            let _ = writeln!(self.io.err, "note: {n}");
        }
    }

    /// Appendix A.3's check, for a command that will touch a Keychain item. With no store,
    /// `switch` and `remove` have no account to touch (§5), so they run none: a fresh machine
    /// gets their no-op or "no account matches" even with a locked keychain. A store created
    /// by another process meanwhile is covered too: the tri-state reads refuse on their own.
    /// `switch`'s offer to add an unmanaged login checks before adding (`switch`).
    fn lock_check(&mut self, command: &Command) -> Result<(), Failure> {
        if command.touches_keychain_only_with_a_store() && self.engine.existing_store()?.is_none() {
            return Ok(());
        }
        self.ensure_unlocked()
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
        if self.can_prompt() && self.io.prompter.confirm(UNLOCK_QUESTION, true) && keychain.unlock()
        {
            return Ok(());
        }
        Err(Failure::Message(
            KIND_KEYCHAIN_LOCKED,
            KEYCHAIN_LOCKED.into(),
        ))
    }

    /// §10.4, with a terminal prompt for an ambiguous email.
    fn resolve(&mut self, input: &str) -> Result<AccountRow, Failure> {
        match self.engine.resolve(input, self.provider_flag.as_ref()) {
            Err(EngineError::Ambiguous { .. }) if self.can_prompt() => {
                let found = self.engine.candidates(input, self.provider_flag.as_ref())?;
                let labels: Vec<String> = found
                    .iter()
                    .map(|r| format!("{} #{} {}", r.provider, r.position, render::name(r)))
                    .collect();
                self.io
                    .prompter
                    .choose("Which account?", &labels)
                    .and_then(|i| found.get(i).cloned())
                    .ok_or_else(cancelled)
            }
            other => Ok(other?),
        }
    }

    fn dispatch(&mut self, command: Command) -> Result<(), Failure> {
        match command {
            Command::List => {
                let lists = self.engine.accounts(self.provider_flag.as_ref())?;
                let engine = &self.engine;
                let names = |id: &str| {
                    engine
                        .provider(&ProviderId::new(id))
                        .map_or_else(|_| id.to_owned(), |p| p.display_name().to_owned())
                };
                let human = render::list_human(&lists, &names);
                self.print(&human, render::list_json(&lists, &self.provider()));
            }
            Command::Status => {
                let provider = self.provider();
                let s = self.engine.status(&provider)?;
                self.print(
                    &render::status_human(&s),
                    render::status_json(&s, provider.as_str()),
                );
            }
            Command::Switch { account, force } => self.switch(account, force)?,
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
            } => {
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
            }
            Command::Remove { account } => {
                let row = self.resolve(&account)?;
                // Decided before the row is gone. Removal never touches the live login
                // (§10.3), so this also says whether the removed login is still the live one.
                let active = self.is_active(&row)?;
                let row = self.engine.remove(&row.id)?;
                let human = format!(
                    "Removed {} (position {}).\n",
                    render::name(&row),
                    row.position
                );
                self.print_view(&human, AccountView { row, active }, None);
            }
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
            } => match (account, name, unset) {
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
                    self.print(&human, json!({"schemaVersion": 1, "aliases": aliases}));
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
            },
            Command::Move { account, position } => {
                let row = self.resolve(&account)?;
                let row = self.engine.move_to(&row.id, position)?;
                let human = format!(
                    "{} is now at position {}.\n",
                    render::email(&row),
                    row.position
                );
                self.print_account(&human, row, None);
            }
        }
        Ok(())
    }

    /// §10.2's token source: `-` reads one line from stdin; none prompts without echo, and
    /// only when a person can answer.
    fn token(&mut self, token: Option<String>) -> Result<String, Failure> {
        match token {
            Some(t) if t == "-" => {
                let mut line = String::new();
                std::io::stdin()
                    .lock()
                    .read_line(&mut line)
                    .map_err(EngineError::Io)?;
                Ok(line)
            }
            Some(t) => Ok(t),
            None if self.can_prompt() => self.io.prompter.secret("Token: ").ok_or_else(cancelled),
            None => Err(Failure::Message(KIND_INVALID_INPUT, TOKEN_MISSING.into())),
        }
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
        self.print(human, render::account_json(&view, created));
    }

    /// An account command's result, with `active` as the engine sees it after the command.
    /// The change has committed by now, so a failure to read `active` never fails it.
    fn print_account(&mut self, human: &str, row: AccountRow, created: Option<bool>) {
        let active = or_inactive(self.is_active(&row), row.position, &row.id);
        self.print_view(human, AccountView { row, active }, created);
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
                if !self.io.prompter.confirm(&question, false) {
                    return Err(cancelled());
                }
                Ok(write(&self.engine, true)?)
            }
            other => Ok(other?),
        }
    }

    fn add(
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

    fn switch(&mut self, account: Option<String>, force: bool) -> Result<(), Failure> {
        let (target, provider) = match &account {
            Some(a) => {
                let row = self.resolve(a)?;
                (SwitchTarget::Account(row.id), row.provider)
            }
            None => (SwitchTarget::Rotation, self.provider()),
        };
        let req = || SwitchRequest {
            provider: provider.clone(),
            target: target.clone(),
            force,
            source: "cli",
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
            if !self
                .io
                .prompter
                .confirm(&format!("Add the current login ({email}) first?"), true)
            {
                return Err(cancelled());
            }
            // With no store, `lock_check` ran no check for this `switch`, and adding reads the
            // live credential and writes the vault: Appendix A.3 applies before it does.
            self.ensure_unlocked()?;
            self.add(provider.clone(), None, None, false)?;
            outcome = self.engine.switch(req())?;
        }
        for w in &outcome.warnings {
            let _ = writeln!(self.io.err, "warning: {w}");
        }
        // As the engine reports it for this write: a file that is the platform's only store is
        // routine and says nothing.
        if let Some(notice) = render::fallback_notice(&outcome) {
            let _ = writeln!(self.io.err, "warning: {notice}");
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
    use super::*;

    /// An environment that sets both test overrides.
    fn both_set(k: &str) -> Option<OsString> {
        match k {
            TEST_KEYCHAIN_DIR => Some("/nonexistent/keychain".into()),
            TEST_PLATFORM => Some("linux".into()),
            _ => None,
        }
    }

    /// Runs under both builds: with `test-support` the overrides are honoured; without it (a
    /// release build) they are ignored, whatever the environment holds.
    #[test]
    fn the_test_overrides_exist_only_with_test_support() {
        let (kc, platform) = test_overrides(&both_set);
        let honoured = cfg!(feature = "test-support");
        assert_eq!(kc.is_some(), honoured);
        assert_eq!(platform, honoured.then_some(Platform::Linux));
    }

    #[test]
    fn an_unreadable_active_flag_after_a_commit_is_inactive_not_an_error() {
        let id = AccountId::from_string("0192");
        let failed = Err(EngineError::Io(std::io::Error::other("store went away")));
        assert!(!or_inactive(failed, 1, &id));
        assert!(or_inactive(Ok(true), 1, &id));
    }
}
