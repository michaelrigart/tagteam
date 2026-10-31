use std::ffi::OsString;
use std::io::{IsTerminal, Write};
use std::sync::Arc;

use tagteam_cc::ClaudeCode;
use tagteam_cc::endpoints::Endpoints;
use tagteam_core::ProviderId;
use tagteam_engine::lazy_http::LazyHttp;
use tagteam_engine::net::UreqHttp;
use tagteam_engine::oracle::{CachingOracle, HttpOracle};
use tagteam_engine::registry::ProviderRegistry;
use tagteam_engine::service::recorded_vars;
use tagteam_engine::session::detect_run_shell;
use tagteam_engine::settings::{self, ColorMode, Settings};
use tagteam_engine::vault::{FileVault, KeychainVault, Vault};
use tagteam_engine::{Engine, EngineConfig, EngineError, EventSource};
use tagteam_provider::http::Http;
use tagteam_provider::liveness::SystemProcessProbe;
use tagteam_provider::process::SystemSpawner;
use tagteam_provider::profile::RunShell;
#[cfg(feature = "test-support")]
use tagteam_provider::security::ProcessRunner;
use tagteam_provider::security::SecurityCli;
use tagteam_provider::{Clock, Env, EnvError, Keychain, Platform, SystemClock};

use crate::prompt::Prompter;

const NO_COLOR: &str = "NO_COLOR";
const FORCE_COLOR: &str = "FORCE_COLOR";

/// Honoured only with the `test-support` feature: a release build never reads them.
#[cfg(any(test, feature = "test-support"))]
const TEST_KEYCHAIN_DIR: &str = "TAGTEAM_TEST_KEYCHAIN_DIR";
#[cfg(any(test, feature = "test-support"))]
const TEST_PLATFORM: &str = "TAGTEAM_TEST_PLATFORM";
#[cfg(any(test, feature = "test-support"))]
const TEST_API_BASE: &str = "TAGTEAM_TEST_API_BASE";
/// A keychain file for the vault alone, and for nothing else: `cargo xtask compat` keeps its
/// test account's vault there while Claude Code's items stay in the login keychain.
#[cfg(any(test, feature = "test-support"))]
const TEST_VAULT_KEYCHAIN: &str = "TAGTEAM_TEST_VAULT_KEYCHAIN";

#[derive(Clone)]
pub struct Context {
    pub env: Env,
    pub keychain: Arc<dyn Keychain>,
    /// The vault's own Keychain, when it is not `keychain`: only a test-support build sets it
    /// (`TAGTEAM_TEST_VAULT_KEYCHAIN`, `cargo xtask compat`). `None` keeps the vault in
    /// `keychain`.
    pub vault_keychain: Option<Arc<dyn Keychain>>,
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
    /// The settings warnings the host has already shown, which a command leaves out of its
    /// own (redesign §13): the TUI's, printed at start-up. The binary's own context holds none.
    pub shown_warnings: Arc<[String]>,
}

#[derive(Default)]
struct Overrides {
    keychain: Option<Arc<dyn Keychain>>,
    vault_keychain: Option<Arc<dyn Keychain>>,
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
    // `/usr/bin/security` bound to that keychain file, as the `real_keychain` tests drive it.
    let vault_keychain = var(TEST_VAULT_KEYCHAIN).map(|file| {
        Arc::new(SecurityCli::with_runner(
            Box::new(ProcessRunner),
            Some(std::path::PathBuf::from(file)),
        )) as Arc<dyn Keychain>
    });
    Overrides {
        keychain,
        vault_keychain,
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
    /// The binary's context, read from the process. Fails when `HOME` cannot place tagteam's
    /// files (§5), before anything else is read.
    pub fn from_process() -> Result<Self, EnvError> {
        let env = Env::from_process()?;
        let o = test_overrides(&|k| std::env::var_os(k));
        let mut ctx = Self {
            env,
            keychain: o.keychain.unwrap_or_else(|| Arc::new(SecurityCli::new())),
            vault_keychain: o.vault_keychain,
            platform: o.platform.unwrap_or_else(Platform::current),
            api_base: o.api_base,
            stdout_terminal: std::io::stdout().is_terminal(),
            no_color_env: env_flag(NO_COLOR),
            force_color_env: env_flag(FORCE_COLOR),
            shown_warnings: Arc::from(Vec::<String>::new()),
        };
        // The variables this build's providers asked for, captured here at the process
        // boundary and never in `run`, so a hand-built `Context` carries exactly the
        // `vars` its test set (§15.1). Building the registry touches no Keychain item.
        let registry = build_registry(&ctx);
        ctx.env.capture_homes(&home_vars(&registry));
        let names = session_vars(&registry);
        ctx.env.capture_vars(&names);
        // Which of the variables a session scrubs are set here, for the launch's warnings
        // (§12.5), which the engine computes from this record. Only their presence is kept,
        // never a value: a value may be a login.
        // `Env`'s `Debug` prints `vars`' names only, so a later capture of values (the proxy
        // URLs below) cannot leak through `{:?}`.
        for name in scrubbed_vars(&registry) {
            if std::env::var_os(&name).is_some() {
                ctx.env.vars.insert(name, OsString::new());
            }
        }
        // D9.1, D10: the variables a service unit records, as this process has them, for
        // `daemon install` and for doctor's comparison with an installed unit. They come after
        // the presence markers above. A provider's home variable can be among both: its value,
        // a directory the Env already holds raw in `homes`, replaces its marker and keeps it
        // present.
        ctx.env.capture_vars(&recorded_vars(&registry));
        Ok(ctx)
    }
}

pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
    pub prompter: &'a mut dyn Prompter,
}

impl Io<'_> {
    pub(crate) fn warn(&mut self, warning: &str) {
        let _ = writeln!(self.err, "warning: {warning}");
    }

    pub(crate) fn note(&mut self, note: &str) {
        note_to(self.err, note);
    }
}

/// `Io::note` for a site that holds `err` apart from the rest of `Io`.
pub(crate) fn note_to(err: &mut dyn Write, note: &str) {
    let _ = writeln!(err, "note: {note}");
}

/// The providers this build registers (§4.5): the one place that names them. Every registry is
/// built here, over the given Keychain and platform, with every endpoint under the test base
/// when one is set.
pub(crate) fn registry(
    keychain: Arc<dyn Keychain>,
    platform: Platform,
    api_base: Option<&str>,
) -> ProviderRegistry {
    let mut cc = ClaudeCode::new(keychain, platform);
    if let Some(base) = api_base {
        cc = cc.with_endpoints(Endpoints::with_base(base));
    }
    ProviderRegistry::new().with(Arc::new(cc))
}

/// The providers this build registers, over the context's Keychain and platform.
pub(crate) fn build_registry(ctx: &Context) -> ProviderRegistry {
    registry(ctx.keychain.clone(), ctx.platform, ctx.api_base.as_deref())
}

/// The provider a command acts on when `default_provider` names none this build has (C-3): the
/// registry's first. An empty registry is a build error, so it panics.
pub(crate) fn fallback_provider(registry: &ProviderRegistry) -> ProviderId {
    registry
        .all()
        .first()
        .expect("a build registers at least one provider")
        .id()
}

/// The variables `Context::from_process` captures into `Env.homes`: every provider's
/// `vars().homes`, in registration order, each once.
pub(crate) fn home_vars(registry: &ProviderRegistry) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for name in registry.all().iter().flat_map(|p| p.vars().homes) {
        if !names.contains(name) {
            names.push(name);
        }
    }
    names
}

/// The variables `Context::from_process` captures into `Env.vars`, at the process boundary
/// alone: every variable `registry`'s providers name a profile with (§4.5 `session_dir_var`),
/// then each provider's `vars().captured`, then `PATH`, which `plan_run` looks the launch
/// command up on (§12.1). Each name comes once.
pub(crate) fn session_vars(registry: &ProviderRegistry) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    let named = registry.all().iter().filter_map(|p| p.session_dir_var());
    let captured = registry
        .all()
        .iter()
        .flat_map(|p| p.vars().captured.iter().copied());
    for name in named.chain(captured).chain(["PATH"]) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The variables a session of `registry`'s providers scrubs (§12.5), by name: the names whose
/// presence `Context::from_process` records for the launch's warnings (§12.5). Claude
/// Code's `CLAUDE_CODE_*_FILE_DESCRIPTOR` names are among them as this process's environment
/// holds them, since `session_env` expands the pattern against it. A name that is not UTF-8
/// cannot be a key of `Env.vars`, so it is left out: it is still scrubbed, without a warning.
///
/// Inside a run shell, the run shell's detection rewrites a recorded
/// `CLAUDE_SECURESTORAGE_CONFIG_DIR` to the outer home's record, or drops it where the outer
/// home had none, so the launch warns about that variable as the outer home set it. That is
/// accepted: `run` launches as from the outer home, and the variable names a
/// directory, never a login.
pub(crate) fn scrubbed_vars(registry: &ProviderRegistry) -> Vec<String> {
    registry
        .all()
        .iter()
        .filter(|p| p.capabilities().sessions)
        .flat_map(|p| p.session_env("").remove)
        .filter_map(|name| name.into_string().ok())
        .collect()
}

/// §12.8: where this process stands. The marker decides the run shell and the environment the
/// engine runs on, detected once at construction. It only detects: the variables are already
/// in `env.vars`, captured at the process boundary (`Context::from_process`).
fn locate(env: Env, registry: &ProviderRegistry) -> (RunShell, Env) {
    detect_run_shell(&env, registry)
}

/// What one lookup of the run shell found: the registry it went through, the run shell
/// (never `RunShell::Unreadable`) and the effective environment.
pub(crate) struct Located {
    pub(crate) registry: ProviderRegistry,
    pub(crate) run_shell: RunShell,
    pub(crate) env: Env,
}

/// §12.8: the run shell is found through the registered providers, so the registry comes
/// first. Under a marker that cannot be read the outer home is unknown, and nothing runs. The
/// marker is read from disk on every call, so a command calls this once.
pub(crate) fn locate_run_shell(ctx: &Context) -> Result<Located, EngineError> {
    let registry = build_registry(ctx);
    let (run_shell, env) = locate(ctx.env.clone(), &registry);
    if let RunShell::Unreadable { marker, detail } = run_shell {
        return Err(EngineError::RunShellUnreadable { marker, detail });
    }
    Ok(Located {
        registry,
        run_shell,
        env,
    })
}

/// The engine for one command and its settings warnings, after the one lookup of the run shell,
/// recording its events under `source` (review refactor §8.7).
pub(crate) fn engine_for(
    ctx: Context,
    provider: Option<&ProviderId>,
    source: EventSource,
) -> Result<(Engine, Vec<String>), EngineError> {
    let located = locate_run_shell(&ctx)?;
    Ok(build_engine(
        ctx,
        located.registry,
        located.run_shell,
        located.env,
        provider,
        source,
    ))
}

/// The settings one command runs with (§6.4), and the providers it was read for.
pub(crate) struct CommandSettings {
    /// The provider the command acts on. Its own tables came first.
    pub(crate) provider: ProviderId,
    /// `default_provider` when this build has it, else the registry's first provider (§13.1;
    /// a provider this build lacks warns and falls back).
    pub(crate) default: ProviderId,
    pub(crate) settings: Settings,
    /// The settings warnings, then the CLI's own for a `default_provider` this build lacks.
    pub(crate) warnings: Vec<String>,
}

/// §13.1's rule 3: without `--provider`, a command acts on `default_provider`
/// when `registered` has it, and on `fallback`, the registry's first provider, otherwise: with a
/// warning when the file names one this build lacks. `choose` is given that default and names
/// the provider the command acts on. `default_provider` sits at the top level, so any provider's
/// read of the file gives it: the file is read for `fallback`, and read again only when `choose`
/// names another provider, whose own tables come first.
pub(crate) fn command_settings(
    env: &Env,
    fallback: &ProviderId,
    registered: &dyn Fn(&ProviderId) -> bool,
    choose: impl FnOnce(&ProviderId) -> ProviderId,
) -> CommandSettings {
    let (mut settings, mut warnings) = Settings::load(env, fallback);
    let configured = settings
        .default_provider
        .clone()
        .unwrap_or_else(|| fallback.clone());
    let (default, unregistered) = if registered(&configured) {
        (configured, None)
    } else {
        let warning = format!(
            "{}: `default_provider` names {configured}, which this build does not have; using {fallback}",
            settings::config_path(env).display()
        );
        (fallback.clone(), Some(warning))
    };
    let provider = choose(&default);
    if provider != *fallback {
        (settings, warnings) = Settings::load(env, &provider);
    }
    warnings.extend(unregistered);
    CommandSettings {
        provider,
        default,
        settings,
        warnings,
    }
}

/// The engine for one command, and the settings warnings for the caller to print (§6.4). The
/// settings are those of the provider the command acts on (`flag`, else the default, as
/// `command_settings` chooses it): its own tables come first. `env` is the effective
/// environment `locate` returned with `run_shell`. The HTTP adapter is built on its first
/// request, never before (§13.5), and the oracle sends through the same one. The engine records its
/// events under `source` (review refactor §8.7).
pub(crate) fn build_engine(
    ctx: Context,
    registry: ProviderRegistry,
    run_shell: RunShell,
    env: Env,
    flag: Option<&ProviderId>,
    source: EventSource,
) -> (Engine, Vec<String>) {
    let vault = match ctx.platform {
        Platform::MacOs => Vault::new(Box::new(KeychainVault::new(
            ctx.vault_keychain.unwrap_or(ctx.keychain),
        ))),
        Platform::Linux => Vault::new(Box::new(FileVault::new(env.data_dir().join("vault")))),
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
    let fallback = fallback_provider(&registry);
    let chosen = command_settings(&env, &fallback, &|p| registry.get(p).is_some(), |default| {
        flag.cloned().unwrap_or_else(|| default.clone())
    });
    let engine = Engine::new(EngineConfig {
        env,
        registry,
        vault,
        // §7.6: asked at most once per credential within one command.
        oracle: Arc::new(CachingOracle::new(HttpOracle::new(
            http.clone(),
            clock.clone(),
        ))),
        clock,
        http,
        default_provider: chosen.default,
        settings: chosen.settings,
        process: Arc::new(SystemProcessProbe),
        spawner: Arc::new(SystemSpawner),
        run_shell,
        source,
    });
    (engine, chosen.warnings)
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

/// The four inputs to the colour decision besides `ui.color`.
#[derive(Clone, Copy)]
pub(crate) struct ColorEnv {
    pub(crate) no_color_flag: bool,
    pub(crate) no_color_env: bool,
    pub(crate) force_color_env: bool,
    pub(crate) stdout_terminal: bool,
}

impl ColorEnv {
    pub(crate) fn of(ctx: &Context, no_color_flag: bool) -> Self {
        Self {
            no_color_flag,
            no_color_env: ctx.no_color_env,
            force_color_env: ctx.force_color_env,
            stdout_terminal: ctx.stdout_terminal,
        }
    }

    /// §13.1 and §6.4: `--no-color` and `NO_COLOR` always turn colour off, `FORCE_COLOR` turns
    /// it on, and otherwise `ui.color` decides, `auto` meaning "stdout is a terminal".
    pub(crate) fn enabled(&self, setting: ColorMode) -> bool {
        if self.no_color_flag || self.no_color_env {
            return false;
        }
        if self.force_color_env {
            return true;
        }
        match setting {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => self.stdout_terminal,
        }
    }
}

#[cfg(test)]
mod tests {
    use tagteam_cc::CLAUDE_CODE;
    use tagteam_provider::FakeKeychain;

    use crate::prompt::Prompter;
    use crate::statusline;

    use super::{
        Arc, ClaudeCode, ColorEnv, ColorMode, Context, Env, Io, OsString, Platform, ProviderId,
        ProviderRegistry, TEST_API_BASE, TEST_KEYCHAIN_DIR, TEST_PLATFORM, TEST_VAULT_KEYCHAIN,
        build_registry, command_settings, fallback_provider, home_vars, is_set, scrubbed_vars,
        session_vars, settings, test_overrides,
    };

    #[test]
    fn the_fallback_provider_is_the_registrys_first() {
        use tagteam_fake::{FAKE_AGENT, FakeAgent};
        let cc = || {
            Arc::new(ClaudeCode::new(
                Arc::new(FakeKeychain::default()),
                Platform::MacOs,
            ))
        };
        let fake_first = ProviderRegistry::new()
            .with(Arc::new(FakeAgent::new()))
            .with(cc());
        assert_eq!(fallback_provider(&fake_first).as_str(), FAKE_AGENT);
        let cc_first = ProviderRegistry::new()
            .with(cc())
            .with(Arc::new(FakeAgent::new()));
        assert_eq!(fallback_provider(&cc_first).as_str(), CLAUDE_CODE);
    }

    /// An environment that sets every test override.
    fn all_set(k: &str) -> Option<OsString> {
        match k {
            TEST_KEYCHAIN_DIR => Some("/nonexistent/keychain".into()),
            TEST_PLATFORM => Some("linux".into()),
            TEST_API_BASE => Some("http://127.0.0.1:9".into()),
            TEST_VAULT_KEYCHAIN => Some("/nonexistent/vault.keychain-db".into()),
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
        assert_eq!(o.vault_keychain.is_some(), honoured);
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
            let env = ColorEnv {
                no_color_flag: flag,
                no_color_env: no,
                force_color_env: force,
                stdout_terminal: terminal,
            };
            assert_eq!(
                env.enabled(setting),
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

    /// A fresh environment under `dir` whose `config.toml` holds `text`.
    fn env_with_config(dir: &std::path::Path, text: &str) -> Env {
        let env = Env::for_test(dir);
        std::fs::create_dir_all(env.config_dir()).unwrap();
        std::fs::write(settings::config_path(&env), text).unwrap();
        env
    }

    fn claude_code_only(p: &ProviderId) -> bool {
        p.as_str() == CLAUDE_CODE
    }

    fn and_other(p: &ProviderId) -> bool {
        [CLAUDE_CODE, "other"].contains(&p.as_str())
    }

    #[test]
    fn with_no_settings_file_a_command_acts_on_claude_code_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let chosen = command_settings(
            &env,
            &ProviderId::new(CLAUDE_CODE),
            &claude_code_only,
            |d| d.clone(),
        );
        assert_eq!(
            (chosen.provider.as_str(), chosen.default.as_str()),
            (CLAUDE_CODE, CLAUDE_CODE)
        );
        assert!(chosen.warnings.is_empty(), "{:?}", chosen.warnings);
        assert!(!env.config_dir().exists());
    }

    #[test]
    fn without_a_default_provider_setting_a_command_acts_on_the_fallback_it_is_given() {
        // C-3: the fallback is the registry's first provider, not a name core or the engine knows.
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let chosen = command_settings(&env, &ProviderId::new("other"), &and_other, |d| d.clone());
        assert_eq!(
            (chosen.provider.as_str(), chosen.default.as_str()),
            ("other", "other")
        );
        assert!(chosen.warnings.is_empty(), "{:?}", chosen.warnings);
    }

    #[test]
    fn without_provider_a_command_acts_on_a_registered_default_provider() {
        // §13.1 rule 3: the default provider's own tables come first (§6.4).
        let dir = tempfile::tempdir().unwrap();
        let env = env_with_config(
            dir.path(),
            "default_provider = \"other\"\n[statusline]\nformat = \"{5h}\"\n\
             [provider.other.statusline]\nformat = \"{7d}\"\n",
        );
        let chosen = command_settings(&env, &ProviderId::new(CLAUDE_CODE), &and_other, |d| {
            d.clone()
        });
        assert_eq!(
            (chosen.provider.as_str(), chosen.default.as_str()),
            ("other", "other")
        );
        assert_eq!(chosen.settings.statusline_format, "{7d}");
        assert!(chosen.warnings.is_empty(), "{:?}", chosen.warnings);
        // `--provider` still wins, with its own tables; the default stays what it is.
        let chosen = command_settings(&env, &ProviderId::new(CLAUDE_CODE), &and_other, |_| {
            ProviderId::new(CLAUDE_CODE)
        });
        assert_eq!(
            (chosen.provider.as_str(), chosen.default.as_str()),
            (CLAUDE_CODE, "other")
        );
        assert_eq!(chosen.settings.statusline_format, "{5h}");
    }

    #[test]
    fn a_default_provider_this_build_lacks_falls_back_to_claude_code_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with_config(
            dir.path(),
            "default_provider = \"other\"\n[provider.other.statusline]\nformat = \"{7d}\"\n",
        );
        let chosen = command_settings(
            &env,
            &ProviderId::new(CLAUDE_CODE),
            &claude_code_only,
            |d| d.clone(),
        );
        assert_eq!(
            (chosen.provider.as_str(), chosen.default.as_str()),
            (CLAUDE_CODE, CLAUDE_CODE)
        );
        assert_eq!(
            chosen.settings.statusline_format,
            settings::DEFAULT_STATUSLINE_FORMAT,
            "claude-code's tables, not other's"
        );
        assert_eq!(
            chosen
                .settings
                .default_provider
                .as_ref()
                .map(ProviderId::as_str),
            Some("other"),
            "the setting itself reads as written"
        );
        let warning = format!(
            "{}: `default_provider` names other, which this build does not have; using claude-code",
            settings::config_path(&env).display()
        );
        assert_eq!(chosen.warnings, [warning.clone()]);
        // An explicit provider, as `--provider` names one, warns all the same.
        let chosen = command_settings(
            &env,
            &ProviderId::new(CLAUDE_CODE),
            &claude_code_only,
            |_| ProviderId::new(CLAUDE_CODE),
        );
        assert_eq!(chosen.default.as_str(), CLAUDE_CODE);
        assert_eq!(chosen.warnings, [warning]);
    }

    #[test]
    fn the_warnings_are_those_of_the_provider_the_command_acts_on_once() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with_config(
            dir.path(),
            "[autoswitch]\nthreshold = 5\n[provider.other.autoswitch]\nthreshold = 120\n",
        );
        let chosen = command_settings(&env, &ProviderId::new(CLAUDE_CODE), &and_other, |_| {
            ProviderId::new("other")
        });
        assert_eq!(chosen.warnings.len(), 2, "{:?}", chosen.warnings);
        assert!(chosen.warnings[0].contains("`provider.other.autoswitch.threshold`"));
        assert!(chosen.warnings[1].contains("`autoswitch.threshold`"));
        let chosen = command_settings(&env, &ProviderId::new(CLAUDE_CODE), &and_other, |d| {
            d.clone()
        });
        assert_eq!(chosen.warnings.len(), 1, "{:?}", chosen.warnings);
        assert!(chosen.warnings[0].contains("`autoswitch.threshold`"));
    }

    #[test]
    fn the_statusline_builds_the_registry_every_other_command_builds() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context {
            env: Env::for_test(dir.path()),
            keychain: Arc::new(tagteam_provider::FakeKeychain::new()),
            vault_keychain: None,
            platform: Platform::MacOs,
            api_base: None,
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
            shown_warnings: Arc::from(Vec::<String>::new()),
        };
        let registered: Vec<ProviderId> =
            build_registry(&ctx).all().iter().map(|p| p.id()).collect();
        let (statusline, _) = statusline::engine(ctx, None);
        assert_eq!(statusline.provider_ids(), registered);
        assert_eq!(
            crate::cli::completed_providers(),
            registered
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_process_boundary_captures_each_provider_s_home_variables() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context {
            env: Env::for_test(dir.path()),
            keychain: Arc::new(FakeKeychain::new()),
            vault_keychain: None,
            platform: Platform::MacOs,
            api_base: None,
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
            shown_warnings: Arc::from(Vec::<String>::new()),
        };
        assert_eq!(
            home_vars(&build_registry(&ctx)),
            ["CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR"]
        );
        assert!(
            ctx.env.homes.is_empty(),
            "only the process boundary captures"
        );
    }

    #[test]
    fn the_process_boundary_captures_every_session_variable_and_claudecode() {
        // `Context::from_process` captures these names, and only it does, so a
        // hand-built `Context` carries exactly the `vars` its test set (§15.1).
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context {
            env: Env::for_test(dir.path()),
            keychain: Arc::new(FakeKeychain::new()),
            vault_keychain: None,
            platform: Platform::MacOs,
            api_base: None,
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
            shown_warnings: Arc::from(Vec::<String>::new()),
        };
        assert_eq!(
            session_vars(&build_registry(&ctx)),
            [
                "CLAUDE_CONFIG_DIR",
                "CLAUDECODE",
                "CLAUDE_CODE_SESSION_ID",
                "PATH"
            ]
        );
        assert!(
            ctx.env.vars.is_empty(),
            "nothing but the process boundary captures"
        );
    }

    #[test]
    fn the_boundary_records_the_variables_a_session_scrubs_and_never_its_home() {
        // The names `Context::from_process` records the presence of, for the
        // launch's environment warnings (§12.5).
        let dir = tempfile::tempdir().unwrap();
        let ctx = Context {
            env: Env::for_test(dir.path()),
            keychain: Arc::new(FakeKeychain::new()),
            vault_keychain: None,
            platform: Platform::MacOs,
            api_base: None,
            stdout_terminal: false,
            no_color_env: false,
            force_color_env: false,
            shown_warnings: Arc::from(Vec::<String>::new()),
        };
        let names = scrubbed_vars(&build_registry(&ctx));
        for name in [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_SESSION_ACCESS_TOKEN",
            "CLAUDE_CODE_HOST_CREDS_FILE",
            "CLAUDE_CODE_SESSIONS_OAUTH_TOKEN",
            "USE_STAGING_OAUTH",
            "CLAUDE_SECURESTORAGE_CONFIG_DIR",
        ] {
            assert!(names.iter().any(|n| n == name), "{name}: {names:?}");
        }
        assert!(
            !names.iter().any(|n| n == "CLAUDE_CONFIG_DIR"),
            "a session sets its home variable, never scrubs it: {names:?}"
        );
        assert!(
            ctx.env.vars.is_empty(),
            "only the process boundary records them"
        );
    }

    /// Answers no prompt: the reporting helpers ask none.
    struct NoPrompts;

    impl Prompter for NoPrompts {
        fn interactive(&self) -> bool {
            false
        }
        fn confirm(&mut self, question: &str, _default_yes: bool) -> bool {
            panic!("unexpected prompt: {question}");
        }
        fn choose(&mut self, question: &str, _options: &[String]) -> Option<usize> {
            panic!("unexpected prompt: {question}");
        }
        fn secret(&mut self, question: &str) -> Option<String> {
            panic!("unexpected prompt: {question}");
        }
    }

    #[test]
    fn io_warn_and_note_write_one_prefixed_line_each() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut io = Io {
            out: &mut out,
            err: &mut err,
            prompter: &mut NoPrompts,
        };
        io.warn("x");
        io.note("y");
        assert_eq!(err, b"warning: x\nnote: y\n");
        assert!(out.is_empty());
    }
}
