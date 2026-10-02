use std::ffi::OsString;
use std::io::{IsTerminal, Write};
use std::sync::Arc;

use serde_json::{Value, json};
use tagteam_cc::ClaudeCode;
use tagteam_cc::endpoints::Endpoints;
use tagteam_cc::live::Platform;
use tagteam_core::autoswitch::Strategy;
use tagteam_core::{AccountId, CLAUDE_CODE, Pace, ProviderId, Window};
use tagteam_engine::collect::CollectMode;
use tagteam_engine::lazy_http::LazyHttp;
use tagteam_engine::lifecycle::{AddOptions, AddTokenOptions};
use tagteam_engine::net::UreqHttp;
use tagteam_engine::oracle::{CachingOracle, HttpOracle};
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::settings::{ColorMode, Settings, parse_bool};
use tagteam_engine::store::AccountRow;
use tagteam_engine::switch::{SwitchReason, SwitchRequest, SwitchTarget, UsageStrategy};
use tagteam_engine::vault::{FileVault, KeychainVault, Vault};
use tagteam_engine::views::{AccountView, StatusView};
use tagteam_engine::{Engine, EngineConfig, EngineError};
use tagteam_provider::http::Http;
use tagteam_provider::security::SecurityCli;
use tagteam_provider::{Clock, Env, Keychain, LockState, SystemClock};

use crate::auto::{
    AutoError, AutoFlags, AutoRun, HumanSink, JsonSink, ThreadSleeper, uniform_jitter,
};
use crate::cli::{AutoStrategyArg, Cli, Command, StrategyArg};
use crate::prompt::Prompter;
use crate::{auto, history, prompt, render, root_guard, statusline};

/// §13.1.
pub(crate) const EXIT_ERROR: i32 = 1;
pub(crate) const EXIT_USAGE: i32 = 2;
/// §13.1: an interrupted command exits 128 + the signal (130 after SIGINT).
const EXIT_SIGNAL_BASE: i32 = 128;

/// `error.type` kinds the CLI raises itself; the engine's come from `EngineError::kind`.
pub(crate) const KIND_USAGE: &str = "usage";
const KIND_ROOT: &str = "root";
const KIND_KEYCHAIN_LOCKED: &str = "keychain-locked";
const KIND_CANCELLED: &str = "cancelled";
const KIND_INVALID_INPUT: &str = "invalid-input";
const KIND_UNMANAGED_ACCOUNT: &str = "unmanaged-account";
/// The engine's kind for the same condition; the CLI raises it with its own message.
const KIND_NO_LIVE_LOGIN: &str = "no-live-login";
/// A provider without the capability a command needs (§4.5).
const KIND_UNSUPPORTED: &str = "unsupported";
/// §11.1: every provider `auto` would drive has its engine running in another process.
const KIND_ENGINE_RUNNING: &str = "engine-running";
/// `auto` found no provider with two switchable accounts.
const KIND_NO_CANDIDATES: &str = "no-candidates";
/// §14.1, Decision 4: every interruption, whichever carrier holds its signal.
pub(crate) const KIND_INTERRUPTED: &str = "interrupted";

/// Appendix A.3. The default keychain is the login keychain, so the hint names its file.
const UNLOCK_QUESTION: &str = "The login keychain is locked (common over SSH). Unlock it now?";
const KEYCHAIN_LOCKED: &str = "the login keychain is locked (common over SSH); run `security unlock-keychain ~/Library/Keychains/login.keychain-db`, then retry";
const CANCELLED: &str = "cancelled";
const INTERRUPTED: &str = "interrupted";
const TOKEN_MISSING: &str = "pass the token as an argument, or `-` to read it from stdin";
const ALIAS_USAGE: &str = "alias takes ACCOUNT NAME, ACCOUNT --unset, or no arguments";
const NO_LIVE_LOGIN: &str =
    "there is no live login; name an account, or log in with `claude` first";
const CSV_AND_JSON: &str = "--csv and --json are two output formats; pass one";
const BAD_SINCE: &str = "--since takes a span like 14d, 12h or 30m";
const STATUSLINE_UNDER_JSON: &str = "statusline prints a line of text; run it without --json";
const NOTHING_TO_SWITCH: &str =
    "auto-switch needs two switchable accounts on a provider; add another with `tagteam add`";
const BAD_THRESHOLD: &str = "--threshold takes a number from 50 to 99.9";
const BAD_INCLUDE: &str = "--include-api-key-accounts takes true, false, 1, 0, yes or no";

const NO_COLOR: &str = "NO_COLOR";
const FORCE_COLOR: &str = "FORCE_COLOR";

/// Honoured only with the `test-support` feature: a release build never reads them.
#[cfg(any(test, feature = "test-support"))]
const TEST_KEYCHAIN_DIR: &str = "TAGTEAM_TEST_KEYCHAIN_DIR";
#[cfg(any(test, feature = "test-support"))]
const TEST_PLATFORM: &str = "TAGTEAM_TEST_PLATFORM";
#[cfg(any(test, feature = "test-support"))]
const TEST_API_BASE: &str = "TAGTEAM_TEST_API_BASE";

pub struct Context {
    pub env: Env,
    pub keychain: Arc<dyn Keychain>,
    pub platform: Platform,
    /// Every endpoint under this base instead of production; only a test-support build sets it.
    pub api_base: Option<String>,
    /// Whether the output `io.out` writes to is a terminal: `ui.color = auto` colours only then.
    /// The binary sets it from its own stdout; a harness capturing the output sets it false.
    pub stdout_terminal: bool,
    /// `NO_COLOR` and `FORCE_COLOR` are set to a non-empty value (no-color.org, force-color.org).
    /// The binary reads them from its environment; a harness sets them, never the test
    /// process's own.
    pub no_color_env: bool,
    pub force_color_env: bool,
}

#[derive(Default)]
struct Overrides {
    keychain: Option<Arc<dyn Keychain>>,
    platform: Option<Platform>,
    api_base: Option<String>,
}

/// The test harness's keychain, platform and endpoint base, read through `var` so a test can
/// supply the environment without mutating the process's.
#[cfg(feature = "test-support")]
fn test_overrides(var: &dyn Fn(&str) -> Option<OsString>) -> Overrides {
    let keychain = var(TEST_KEYCHAIN_DIR).map(|d| {
        Arc::new(tagteam_provider::FileKeychain::new(
            std::path::PathBuf::from(d),
        )) as Arc<dyn Keychain>
    });
    let platform = match var(TEST_PLATFORM).as_deref().and_then(|v| v.to_str()) {
        Some("linux") => Some(Platform::Linux),
        Some("macos") => Some(Platform::MacOs),
        _ => None,
    };
    let api_base = var(TEST_API_BASE).and_then(|v| v.into_string().ok());
    Overrides {
        keychain,
        platform,
        api_base,
    }
}

/// A release build has no test overrides, whatever the environment holds.
#[cfg(not(feature = "test-support"))]
fn test_overrides(_var: &dyn Fn(&str) -> Option<OsString>) -> Overrides {
    Overrides::default()
}

impl Context {
    pub fn from_process() -> Self {
        let o = test_overrides(&|k| std::env::var_os(k));
        Self {
            env: Env::from_process(),
            keychain: o.keychain.unwrap_or_else(|| Arc::new(SecurityCli::new())),
            platform: o.platform.unwrap_or_else(Platform::current),
            api_base: o.api_base,
            stdout_terminal: std::io::stdout().is_terminal(),
            no_color_env: env_flag(NO_COLOR),
            force_color_env: env_flag(FORCE_COLOR),
        }
    }
}

pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub prompter: &'a mut dyn Prompter,
}

/// The engine for one command, and the settings warnings for the caller to print (§6.4). The
/// settings are those of `provider`, the one the command resolves (`--provider`, else the
/// default): its own tables come first. The HTTP adapter is built on its first request, never
/// before (§13.5), and the oracle sends through the same one.
fn build_engine(ctx: Context, provider: &ProviderId) -> (Engine, Vec<String>) {
    let mut cc = ClaudeCode::new(ctx.keychain.clone(), ctx.platform);
    if let Some(base) = &ctx.api_base {
        cc = cc.with_endpoints(Endpoints::with_base(base));
    }
    let vault = match ctx.platform {
        Platform::MacOs => Vault::new(Box::new(KeychainVault::new(ctx.keychain))),
        Platform::Linux => Vault::new(Box::new(FileVault::new(ctx.env.data_dir().join("vault")))),
    };
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    // `api_base` only ever carries a test base: it is sent to directly, never through whatever
    // proxy the machine's environment names, so test traffic cannot leave the machine.
    let direct = ctx.api_base.is_some();
    let http: Arc<dyn Http> = Arc::new(LazyHttp::new(move || -> Arc<dyn Http> {
        Arc::new(if direct {
            UreqHttp::direct()
        } else {
            UreqHttp::new()
        })
    }));
    let default_provider = ProviderId::new(CLAUDE_CODE);
    let (settings, warnings) = Settings::load(&ctx.env, provider);
    let engine = Engine::new(EngineConfig {
        env: ctx.env,
        registry: ProviderRegistry::new().with(Arc::new(cc)),
        vault,
        // §7.6: asked at most once per credential within one command.
        oracle: Arc::new(CachingOracle::new(HttpOracle::new(
            http.clone(),
            clock.clone(),
        ))),
        clock,
        http,
        default_provider,
        settings,
    });
    (engine, warnings)
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
fn model_list(arg: &str) -> Vec<String> {
    arg.split(',')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `auto`'s flags as the loop takes them (§11.4); each is clamped there (§6.4). clap reads `nan`
/// and `inf` as numbers, so `--threshold` must be finite. `--include-api-key-accounts` takes
/// §6.4's booleans.
fn auto_flags(
    threshold: Option<f64>,
    interval: Option<i64>,
    cooldown: Option<i64>,
    strategy: Option<AutoStrategyArg>,
    model: Option<String>,
    include_api_key_accounts: Option<String>,
) -> Result<AutoFlags, Failure> {
    if threshold.is_some_and(|t| !t.is_finite()) {
        return Err(Failure::Usage(BAD_THRESHOLD.into()));
    }
    let include_api_key_accounts = match include_api_key_accounts.as_deref() {
        Some(v) => Some(parse_bool(v).ok_or_else(|| Failure::Usage(BAD_INCLUDE.into()))?),
        None => None,
    };
    Ok(AutoFlags {
        threshold,
        interval_s: interval,
        cooldown_s: cooldown,
        strategy: strategy.map(|s| match s {
            AutoStrategyArg::Best => Strategy::Best,
            AutoStrategyArg::ConsumeFirst => Strategy::ConsumeFirst,
        }),
        models: model.as_deref().map(model_list),
        include_api_key_accounts,
    })
}

/// A provider's display name, or its ID for one this build does not register.
fn display_name(engine: &Engine, provider: &ProviderId) -> String {
    engine
        .provider(provider)
        .map_or_else(|_| provider.to_string(), |p| p.display_name().to_owned())
}

/// How `auto` fails as a command, each provider named by `name`. Consume-first named for a
/// provider without a long window is a usage error, exit 2, as every bad flag value is (§4.5).
fn auto_failure(e: AutoError, name: &dyn Fn(&ProviderId) -> String) -> Failure {
    match e {
        AutoError::AlreadyRuns(providers) => {
            let names: Vec<String> = providers.iter().map(name).collect();
            Failure::Message(
                KIND_ENGINE_RUNNING,
                format!("auto-switch already runs for {}", names.join(", ")),
            )
        }
        AutoError::NothingToSwitch => {
            Failure::Message(KIND_NO_CANDIDATES, NOTHING_TO_SWITCH.into())
        }
        AutoError::NoLongWindow(p) => Failure::Usage(format!(
            "--strategy consume-first needs a long usage window to rank by, and {} has none",
            name(&p)
        )),
        AutoError::Engine(e) => e.into(),
    }
}

/// Whether a colour variable (`NO_COLOR`, `FORCE_COLOR`) is in force: set to a non-empty value
/// (no-color.org, force-color.org). An empty one is as good as unset.
fn is_set(value: Option<OsString>) -> bool {
    value.is_some_and(|v| !v.is_empty())
}

/// The environment variable `name`, by `is_set`'s rule.
pub(crate) fn env_flag(name: &str) -> bool {
    is_set(std::env::var_os(name))
}

/// §13.1 and §6.4: `--no-color` and `NO_COLOR` always turn colour off, `FORCE_COLOR` turns it
/// on, and otherwise `ui.color` decides, `auto` meaning "stdout is a terminal".
fn color_enabled(
    flag_off: bool,
    no_color: bool,
    force_color: bool,
    setting: ColorMode,
    stdout_terminal: bool,
) -> bool {
    if flag_off || no_color {
        return false;
    }
    if force_color {
        return true;
    }
    match setting {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => stdout_terminal,
    }
}

/// §13.2: the one object `--json` prints for any error.
pub(crate) fn error_json(kind: &str, message: &str) -> Value {
    json!({"schemaVersion": 1, "error": {"type": kind, "message": message}})
}

struct App<'a, 'b> {
    engine: Engine,
    json: bool,
    /// `Context::stdout_terminal`, `no_color_env` and `force_color_env`.
    stdout_terminal: bool,
    no_color_env: bool,
    force_color_env: bool,
    /// `--no-color`.
    no_color: bool,
    provider_flag: Option<ProviderId>,
    /// The Keychain Appendix A.3's lock check asks: macOS only, since Linux has none.
    keychain: Option<Arc<dyn Keychain>>,
    io: &'a mut Io<'b>,
}

/// How a command ended (§13.1, §14.1).
enum Ended {
    /// It finished with this exit code; its output, and any error, are written.
    Code(i32),
    /// It stopped at a cancellation point after this signal; nothing is reported yet.
    Interrupted(i32),
}

/// Runs one command and returns its exit code (§13.1). A signal the command met at a
/// cancellation point ends it with 128 + the signal and the `interrupted` error (Decision 4);
/// one it never met leaves its output and exit code alone and is reported on stderr as too
/// late (Decision 6).
pub fn run(cli: Cli, ctx: Context, io: &mut Io<'_>) -> i32 {
    let json = cli.json;
    let name = cli.command.as_ref().map_or("list", command_name);
    // §11.4: a signal is how the `auto` loop ends, so it is never too late for it.
    let stops_on_a_signal = matches!(cli.command, Some(Command::Auto { once: false, .. }));
    let cancel = ctx.env.cancel.clone();
    match run_command(cli, ctx, io) {
        Ended::Interrupted(signal) => {
            fail(io, json, KIND_INTERRUPTED, INTERRUPTED);
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
    }
}

fn run_command(cli: Cli, ctx: Context, io: &mut Io<'_>) -> Ended {
    let color = !cli.no_color && !ctx.no_color_env;
    init_logging(cli.debug, color);
    let json = cli.json;
    if let Err(msg) = root_guard::refuse_root() {
        return Ended::Code(fail(io, json, KIND_ROOT, &msg));
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
    let command = cli.command.unwrap_or(Command::List);
    let keychain = (ctx.platform == Platform::MacOs).then(|| ctx.keychain.clone());
    let (stdout_terminal, no_color_env, force_color_env) =
        (ctx.stdout_terminal, ctx.no_color_env, ctx.force_color_env);
    let provider_flag = cli.provider.map(ProviderId::new);
    let resolved = provider_flag
        .clone()
        .unwrap_or_else(|| ProviderId::new(CLAUDE_CODE));
    let (engine, warnings) = build_engine(ctx, &resolved);
    for w in &warnings {
        let _ = writeln!(io.err, "warning: {w}");
    }
    let mut app = App {
        engine,
        json,
        stdout_terminal,
        no_color_env,
        force_color_env,
        no_color: cli.no_color,
        provider_flag,
        keychain,
        io,
    };
    if let Some(p) = &app.provider_flag {
        if let Err(e) = app.engine.provider(p) {
            return Ended::Code(fail(app.io, json, e.kind(), &e.to_string()));
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
        Ok(code) => Ended::Code(code),
        Err(Failure::Engine(e)) => match e.signal() {
            Some(signal) => Ended::Interrupted(signal),
            None => Ended::Code(fail(app.io, json, e.kind(), &e.to_string())),
        },
        Err(Failure::Message(kind, m)) => Ended::Code(fail(app.io, json, kind, &m)),
        Err(Failure::Usage(m)) => {
            fail(app.io, json, KIND_USAGE, &m);
            Ended::Code(EXIT_USAGE)
        }
    }
}

/// The command's name in its canonical spelling, for the late notice (Decision 6).
fn command_name(command: &Command) -> &'static str {
    match command {
        Command::List => "list",
        Command::Status => "status",
        Command::Switch { .. } => "switch",
        Command::Add { .. } => "add",
        Command::AddToken { .. } => "add-token",
        Command::Remove { .. } => "remove",
        Command::Disable { .. } => "disable",
        Command::Enable { .. } => "enable",
        Command::Alias { .. } => "alias",
        Command::Move { .. } => "move",
        Command::History { .. } => "history",
        Command::Statusline { .. } => "statusline",
        Command::Auto { .. } => "auto",
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

/// §13.5's fast path, taken before `build_engine`: no lock check, no settings warnings (a status
/// bar has nowhere to show them), and an engine walled off from the Keychain and the network
/// (`statusline::engine`). `main_with_args` has already drained stdin.
fn run_statusline(
    ctx: Context,
    io: &mut Io<'_>,
    json: bool,
    no_color: bool,
    provider: Option<String>,
    print_config: bool,
) -> i32 {
    if json {
        fail(io, true, KIND_USAGE, STATUSLINE_UNDER_JSON);
        return EXIT_USAGE;
    }
    let provider = provider.map_or_else(|| ProviderId::new(CLAUDE_CODE), ProviderId::new);
    let (no_color_env, force_color_env) = (ctx.no_color_env, ctx.force_color_env);
    let (engine, _http, _keychain) = statusline::engine(ctx, &provider);
    let result = statusline_supported(&engine, &provider).and_then(|()| {
        if print_config {
            let _ = writeln!(io.err, "{}", statusline::config_hint(engine.env()));
            return Ok(statusline::config_snippet());
        }
        let view = engine.statusline(&provider)?;
        let settings = engine.settings();
        // The line goes to Claude Code, which renders ANSI colour but is never a terminal, so
        // `auto` colours it: the same rule as `list`, with the terminal test taken as met.
        let colour = color_enabled(
            no_color,
            no_color_env,
            force_color_env,
            settings.color,
            true,
        );
        Ok(statusline::line(
            &view,
            &settings.statusline_format,
            engine.now_ms() / 1000,
            colour,
        ))
    });
    match result {
        Ok(text) => {
            let _ = write!(io.out, "{text}");
            0
        }
        Err(Failure::Engine(e)) => fail(io, false, e.kind(), &e.to_string()),
        Err(Failure::Message(kind, m)) => fail(io, false, kind, &m),
        Err(Failure::Usage(m)) => {
            fail(io, false, KIND_USAGE, &m);
            EXIT_USAGE
        }
    }
}

/// §4.5: `statusline` refuses for a provider without the capability.
fn statusline_supported(engine: &Engine, provider: &ProviderId) -> Result<(), Failure> {
    let p = engine.provider(provider)?;
    match statusline::unsupported(p.capabilities(), p.display_name()) {
        Some(message) => Err(Failure::Message(KIND_UNSUPPORTED, message)),
        None => Ok(()),
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

    /// §14.1, Decision 5: a prompt is a cancellation point. One a signal cut short has
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

    fn notices(&mut self, notices: &[String]) {
        for n in notices {
            let _ = writeln!(self.io.err, "note: {n}");
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
            let _ = writeln!(self.io.err, "warning: {w}");
        }
        Ok(())
    }

    /// Now, in epoch seconds, by the engine's clock: what countdowns and ages count from.
    fn now_s(&self) -> i64 {
        self.engine.now_ms().div_euclid(1000)
    }

    /// Whether `list` and `status` colour their percentages (§13.1).
    fn color(&self) -> bool {
        color_enabled(
            self.no_color,
            self.no_color_env,
            self.force_color_env,
            self.engine.settings().color,
            self.stdout_terminal,
        )
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
        if self.can_prompt() {
            let yes = self.io.prompter.confirm(UNLOCK_QUESTION, true);
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

    /// §10.4, with a terminal prompt for an ambiguous email.
    fn resolve(&mut self, input: &str) -> Result<AccountRow, Failure> {
        match self.engine.resolve(input, self.provider_flag.as_ref()) {
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
                    .ok_or_else(cancelled)
            }
            other => Ok(other?),
        }
    }

    /// Runs `command` and returns its exit code: 0, except for `auto` (§11.4).
    fn dispatch(&mut self, command: Command) -> Result<i32, Failure> {
        match command {
            Command::List => {
                // §8.3: every listed account is offered to the collector, which fetches only
                // those that are due; the views are then read with whatever it recorded.
                let listed = self.engine.accounts(self.provider_flag.as_ref())?;
                let ids: Vec<AccountId> = listed
                    .iter()
                    .flat_map(|l| l.accounts.iter().map(|v| v.row.id.clone()))
                    .collect();
                self.collect(ids)?;
                let lists = self.engine.accounts(self.provider_flag.as_ref())?;
                let (now_s, color) = (self.now_s(), self.color());
                let engine = &self.engine;
                let names = |id: &str| {
                    engine
                        .provider(&ProviderId::new(id))
                        .map_or_else(|_| id.to_owned(), |p| p.display_name().to_owned())
                };
                let human = render::list_human(&lists, &names, now_s, color);
                let json = render::list_json(&lists, &self.provider(), &render_usage(engine));
                self.print(&human, json);
            }
            Command::Status => {
                let provider = self.provider();
                // §8.3: `status` collects the live account only.
                if let StatusView::Managed { account, .. } = self.engine.status(&provider)? {
                    self.collect(vec![account.row.id])?;
                }
                let s = self.engine.status(&provider)?;
                let (now_s, color) = (self.now_s(), self.color());
                let human = render::status_human(&s, now_s, color);
                let json = render::status_json(&s, provider.as_str(), &render_usage(&self.engine));
                self.print(&human, json);
            }
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
                let view = self.engine.account_view(row, active);
                self.print_view(&human, view, None);
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
            Command::History {
                account,
                window,
                since,
                csv,
            } => self.history(account, window, &since, csv)?,
            Command::Statusline { .. } => unreachable!("run answers statusline before dispatch"),
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
                return self.auto(AutoRun {
                    provider: self.provider_flag.clone(),
                    flags,
                    once,
                    dry_run,
                });
            }
        }
        Ok(0)
    }

    /// §11: `auto`. Like every command that changes the live login it refuses inside a run
    /// shell, except with `--dry-run` (§11.1); `run_command` has run the Keychain check. Events
    /// go to stdout as human lines or JSONL (§11.4), warnings and errors to stderr.
    fn auto(&mut self, run: AutoRun) -> Result<i32, Failure> {
        if !run.dry_run && self.engine.env().inside_run_shell() {
            return Err(EngineError::InsideRunShell.into());
        }
        let several = auto::providers(&self.engine, run.provider.as_ref())?.len() > 1;
        let color = self.color();
        let engine = &self.engine;
        let now_ms = || engine.now_ms();
        let names = |p: &ProviderId| display_name(engine, p);
        let Io { out, err, .. } = &mut *self.io;
        let ended = if self.json {
            let sink = JsonSink::new(&mut **out, &now_ms).stopping(engine.cancel());
            auto::run_loop(engine, &run, &sink, &ThreadSleeper, &mut uniform_jitter)
        } else {
            let sink = HumanSink::new(&mut **out, &mut **err, &now_ms, &names, color, several)
                .stopping(engine.cancel());
            auto::run_loop(engine, &run, &sink, &ThreadSleeper, &mut uniform_jitter)
        };
        ended.map_err(|e| auto_failure(e, &names))
    }

    /// §10.2's token source: `-` reads one line from stdin; none prompts without echo, and
    /// only when a person can answer.
    fn token(&mut self, token: Option<String>) -> Result<String, Failure> {
        match token {
            Some(t) if t == "-" && std::io::stdin().is_terminal() => {
                // A plain read would sit through Ctrl-C until Enter (the handler restarts it).
                let line =
                    prompt::read_terminal_line(self.engine.cancel()).map_err(EngineError::Io)?;
                self.after_prompt()?;
                line.ok_or_else(cancelled)
            }
            Some(t) if t == "-" => {
                // Read in slices the token ends, and checked again after the read: a signal that
                // landed during it reports the interruption, never the input it cut short.
                let line =
                    prompt::read_piped_line(self.engine.cancel()).map_err(EngineError::Io)?;
                self.after_prompt()?;
                line.ok_or_else(cancelled)
            }
            Some(t) => Ok(t),
            None if self.can_prompt() => {
                let token = self.io.prompter.secret("Token: ");
                self.after_prompt()?;
                token.ok_or_else(cancelled)
            }
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
        let json = render::account_json(&view, created, &render_usage(&self.engine));
        self.print(human, json);
    }

    /// An account command's result, with `active` as the engine sees it after the command.
    /// The change has committed by now, so a failure to read `active` never fails it.
    fn print_account(&mut self, human: &str, row: AccountRow, created: Option<bool>) {
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

    /// §9: a direct switch to ACCOUNT, a bare rotation, or a usage strategy (§9.3) with its
    /// `--model` override. clap has already refused `--strategy` with an ACCOUNT and `--model`
    /// without `--strategy` (Decision 7).
    fn switch(
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
    /// §13.4: reads `usage_samples` only, so it never fetches. ACCOUNT defaults to the live
    /// login's account; without `--window`, only the windows that count for switching show.
    fn history(
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
            StatusView::NoLogin => Err(Failure::Message(KIND_NO_LIVE_LOGIN, NO_LIVE_LOGIN.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment that sets every test override.
    fn all_set(k: &str) -> Option<OsString> {
        match k {
            TEST_KEYCHAIN_DIR => Some("/nonexistent/keychain".into()),
            TEST_PLATFORM => Some("linux".into()),
            TEST_API_BASE => Some("http://127.0.0.1:9".into()),
            _ => None,
        }
    }

    /// Runs under both builds: with `test-support` the overrides are honoured; without it (a
    /// release build) they are ignored, whatever the environment holds.
    #[test]
    fn the_test_overrides_exist_only_with_test_support() {
        let o = test_overrides(&all_set);
        let honoured = cfg!(feature = "test-support");
        assert_eq!(o.keychain.is_some(), honoured);
        assert_eq!(o.platform, honoured.then_some(Platform::Linux));
        assert_eq!(
            o.api_base.as_deref(),
            honoured.then_some("http://127.0.0.1:9")
        );
    }

    #[test]
    fn colour_is_off_under_no_color_forced_by_force_color_and_else_the_settings() {
        use ColorMode::{Always, Auto, Never};
        // (--no-color, NO_COLOR, FORCE_COLOR, ui.color, stdout is a terminal) → colour
        let cases = [
            ((false, false, false, Auto, true), true),
            ((false, false, false, Auto, false), false),
            ((false, false, false, Always, false), true),
            ((false, false, false, Never, true), false),
            ((false, false, true, Never, false), true),
            ((false, true, true, Always, true), false),
            ((true, false, true, Always, true), false),
        ];
        for ((flag, no, force, setting, terminal), want) in cases {
            assert_eq!(
                color_enabled(flag, no, force, setting, terminal),
                want,
                "{flag} {no} {force} {setting:?} {terminal}"
            );
        }
    }

    #[test]
    fn an_empty_colour_variable_is_as_good_as_unset() {
        assert!(!is_set(None));
        assert!(!is_set(Some(OsString::new())));
        assert!(is_set(Some("1".into())));
        assert!(is_set(Some("0".into())), "any non-empty value counts");
    }

    #[test]
    fn an_unreadable_active_flag_after_a_commit_is_inactive_not_an_error() {
        let id = AccountId::from_string("0192");
        let failed = Err(EngineError::Io(std::io::Error::other("store went away")));
        assert!(!or_inactive(failed, 1, &id));
        assert!(or_inactive(Ok(true), 1, &id));
    }

    #[test]
    fn the_late_notice_names_each_command_as_it_is_typed() {
        use clap::Parser;
        let cases: [&[&str]; 15] = [
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
            &["statusline"],
            &["auto"],
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

    #[test]
    fn consume_first_named_for_a_provider_without_a_long_window_is_a_usage_error() {
        // §4.5. FakeAgent names no long window; `run_loop` refuses the combination before any
        // engine starts, and the command exits 2 in the usage error's shape.
        use tagteam_fake::FakeAgent;
        use tagteam_provider::Provider;
        let fake = FakeAgent::new();
        assert_eq!(fake.primary_long_window(), None);
        let name = |_: &ProviderId| fake.display_name().to_owned();
        let Failure::Usage(message) = auto_failure(AutoError::NoLongWindow(fake.id()), &name)
        else {
            panic!("not a usage error");
        };
        assert_eq!(
            message,
            "--strategy consume-first needs a long usage window to rank by, and FakeAgent has none"
        );
    }
}
