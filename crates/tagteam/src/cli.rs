use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use clap::builder::{PossibleValue, StringValueParser, TypedValueParser};
use clap::{Parser, Subcommand, ValueEnum};
use tagteam_core::CLAUDE_CODE;
use tagteam_engine::settings::KEYS;

/// No `Debug`: `add-token` carries a secret, and a derived `Debug` would print it.
#[derive(Parser)]
#[command(
    name = "tagteam",
    version,
    about = "Multi-account switcher for AI coding agent CLIs"
)]
pub struct Cli {
    /// Print exactly one JSON object on stdout
    #[arg(long, global = true)]
    pub json: bool,
    /// Log diagnostics to stderr
    #[arg(long, global = true)]
    pub debug: bool,
    /// Disable colour
    #[arg(long = "no-color", global = true)]
    pub no_color: bool,
    /// The agent CLI to act on (default: the default_provider setting, else claude-code)
    #[arg(short = 'p', long, global = true, value_name = "PROVIDER")]
    pub provider: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// List stored accounts
    #[command(visible_alias = "ls")]
    List,
    /// Show the live account
    Status,
    /// Switch to the next account, to ACCOUNT, or to the one a usage strategy picks
    Switch {
        account: Option<String>,
        /// Activate even over an unmanaged live login, displacing it
        #[arg(long)]
        force: bool,
        /// best or next-available
        #[arg(long, value_enum, conflicts_with = "account")]
        strategy: Option<StrategyArg>,
        /// Model limits that count, comma-separated, or `all`
        #[arg(long, requires = "strategy")]
        model: Option<String>,
    },
    /// Store the current Claude Code login
    Add {
        #[arg(long)]
        position: Option<u32>,
        #[arg(long)]
        alias: Option<String>,
        /// Replace an account already at --position without asking
        #[arg(long)]
        yes: bool,
    },
    /// Store an API key or a setup token (`-` reads it from stdin)
    AddToken {
        token: Option<String>,
        #[arg(long)]
        position: Option<u32>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        alias: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Delete a stored account (never the live login)
    #[command(visible_alias = "rm")]
    Remove { account: String },
    /// Hold an account out of automatic selection
    Disable { account: String },
    /// Return an account to automatic selection
    Enable { account: String },
    /// Set (ACCOUNT NAME), clear (ACCOUNT --unset) or list aliases
    Alias {
        account: Option<String>,
        name: Option<String>,
        #[arg(long)]
        unset: bool,
    },
    /// Move an account to POSITION, swapping if it is taken
    Move { account: String, position: u32 },
    /// Switch automatically before a rate limit, until stopped (Ctrl-C)
    ///
    /// Runs one engine per provider with two switchable accounts (or only --provider's), each
    /// on its own schedule, and prints a line per tick. --json prints one event per line:
    /// {"schemaVersion":1,"event":<kind>,"ts":"…Z","provider":…, …}. --once ticks once and
    /// exits 0 switched, 1 error, 2 no action, 3 blocked.
    Auto {
        /// Tick once per provider and exit with its outcome
        #[arg(long)]
        once: bool,
        /// Decide and report, but switch nothing and write no auto-switch state
        #[arg(long = "dry-run")]
        dry_run: bool,
        /// Switch away above this percentage of usage (50–99.9)
        #[arg(long, allow_negative_numbers = true)]
        threshold: Option<f64>,
        /// Seconds between ticks (15–3600)
        #[arg(long, allow_negative_numbers = true)]
        interval: Option<i64>,
        /// Seconds after an automatic switch before another proactive one (0–86400)
        #[arg(long, allow_negative_numbers = true)]
        cooldown: Option<i64>,
        /// best or consume-first
        #[arg(long, value_enum)]
        strategy: Option<AutoStrategyArg>,
        /// Model limits that count, comma-separated, or `all`
        #[arg(long)]
        model: Option<String>,
        /// Fall back to API-key accounts at the limit: true, false, 1, 0, yes or no
        #[arg(long = "include-api-key-accounts", value_name = "BOOL")]
        include_api_key_accounts: Option<String>,
    },
    /// Usage history: burn rate, and when each window runs out
    ///
    /// Reads the stored samples only; it never fetches. With no ACCOUNT it shows the live
    /// login's account, and with no --window the windows that count for switching. --json
    /// prints {schemaVersion, provider, account: {number, id, email}, windows: [{key, label,
    /// kind, pct, resetsAt, samples: [{fetchedAt, pct, resetsAt}], ratePerHour, expectedPct,
    /// aheadOfPace, projectedExhaustionAt, willLastToReset, projectionMethod}]}, with times in
    /// ISO 8601 UTC.
    History {
        account: Option<String>,
        /// One window: 5h, 7d, spend, or a model name
        #[arg(long)]
        window: Option<String>,
        /// How far back: 14d, 12h or 30m
        #[arg(long, default_value = "7d")]
        since: String,
        /// Print the raw samples as CSV
        #[arg(long)]
        csv: bool,
    },
    /// Map PATH (default: here) to ACCOUNT for its provider; with no arguments, list mappings
    ///
    /// `tagteam run` in PATH or below it, and so the `shell-init` wrapper, launches ACCOUNT
    /// there. PATH is stored canonical, and a directory holds one mapping per provider. --json
    /// prints {schemaVersion, ok, mapping: {path, provider, number, id, email, alias?,
    /// addedAt}}, and the list {schemaVersion, mappings: [...]}, with times in ISO 8601 UTC.
    Map {
        account: Option<String>,
        path: Option<PathBuf>,
    },
    /// Remove PATH's mappings (default: here): every provider's, or only --provider's
    ///
    /// --json prints {schemaVersion, ok, path, removed}.
    Unmap { path: Option<PathBuf> },
    /// Print the shell function that runs claude through `tagteam run`
    ///
    /// zsh: add `eval "$(tagteam shell-init zsh)"` to ~/.zshrc. bash: the same line, with
    /// bash, in ~/.bashrc. fish: add `tagteam shell-init fish | source` to
    /// ~/.config/fish/config.fish.
    ShellInit { shell: ShellArg },
    /// One line for Claude Code's status bar
    Statusline {
        /// Print the settings.json snippet that sets it up
        #[arg(long = "print-config")]
        print_config: bool,
    },
    /// Run the agent as ACCOUNT, in a session beside the default login
    ///
    /// With no ACCOUNT, the nearest mapped ancestor of this directory decides (`tagteam map`),
    /// and with none the agent runs as it would without tagteam. Everything after `--` goes to
    /// the agent untouched; tagteam's own options, `--json` and `--provider` included, come
    /// before it. `--json` covers only errors before the agent starts: its output is its own.
    Run {
        account: Option<String>,
        /// Refuse wherever the agent would otherwise run without a session
        #[arg(long)]
        require_session: bool,
        /// Arguments for the agent, after `--`.
        #[arg(last = true)]
        args: Vec<OsString>,
    },
    /// The settings in config.toml
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// List credentials tagteam set aside rather than overwrite, or delete them
    ///
    /// A switch saves a live login it would otherwise overwrite, and any other credential that
    /// was not tagteam's to keep, as a file in tagteam's data directory. tagteam never reads one
    /// back: restoring one is manual. --purge deletes the entries named, after a confirmation
    /// or with --yes. --json prints {schemaVersion, dir, displaced: [{id, provider, at, reason,
    /// fingerprint, identity, account, file, recorded}]}, with times in ISO 8601 UTC, and for a
    /// purge {schemaVersion, ok, deleted: [id]}.
    Displaced {
        /// Delete these entries, each file and its row; they cannot be recovered
        #[arg(long, num_args = 1.., value_name = "ID")]
        purge: Vec<String>,
        /// Delete without asking
        #[arg(long)]
        yes: bool,
    },
}

/// `switch --strategy` (§9.3): the strategies that rank accounts by usage.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum StrategyArg {
    /// The candidate with the most headroom, if it has more than the live account
    Best,
    /// The next account in rotation that is not at its limit
    NextAvailable,
}

/// `auto --strategy` (§6.4's `autoswitch.strategy`).
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum AutoStrategyArg {
    /// The candidate with the most headroom, past the hysteresis
    Best,
    /// The candidate whose weekly window resets soonest, while below the threshold
    ConsumeFirst,
}

/// The shells `shell-init` writes for (§12.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ShellArg {
    Zsh,
    Bash,
    Fish,
}

impl Command {
    /// Whether the command reads or writes a Keychain item on macOS, and so runs the lock check
    /// first (Appendix A.3). `list` and `status` are not among them although they collect usage
    /// and may read Keychain items: by a plan ruling they degrade to `keychain_unavailable`
    /// rows rather than run the lock check, so a script gets rows, not a `keychain-locked`
    /// failure. `history` and `statusline` read only the store and `~/.claude.json`, and the
    /// rest need no Keychain item. A recovery under their mutation lock (Task 21) only reads the
    /// Keychain, tri-state, and leaves what it cannot decide to the next command that checks.
    /// `auto` checks before its first tick, a dry run too (§11.1): every tick reads its
    /// accounts' items, and a real one switches.
    /// `run` checks for itself, and only once it knows it launches a session: plain `claude`
    /// touches no item, so an unmapped directory never waits on an unlock prompt (§12.7).
    pub fn touches_keychain(&self) -> bool {
        matches!(
            self,
            Command::Add { .. }
                | Command::AddToken { .. }
                | Command::Switch { .. }
                | Command::Remove { .. }
                | Command::Auto { .. }
        )
    }

    /// Of those, the ones that reach a Keychain item only through a stored account, so with
    /// no store they touch none (§5): there is nothing to activate, delete or switch between.
    pub fn touches_keychain_only_with_a_store(&self) -> bool {
        matches!(
            self,
            Command::Switch { .. } | Command::Remove { .. } | Command::Auto { .. }
        )
    }
}

/// `tagteam config` (§6.4).
#[derive(Subcommand)]
pub enum ConfigAction {
    /// Every setting, with its value and where the value comes from
    List,
    /// One setting's value
    Get {
        /// A setting, such as autoswitch.threshold or provider.claude-code.autoswitch.models
        #[arg(value_parser = ConfigKeyParser, hide_possible_values = true)]
        key: String,
    },
    /// Set KEY to VALUE in config.toml
    ///
    /// Numbers are written as typed, and booleans (also 1/0 and yes/no) as true or false. A
    /// list is comma-separated names, and '' is an empty list. --provider, or a provider.<id>.
    /// prefix, names a provider's own entry.
    Set {
        #[arg(value_parser = ConfigKeyParser, hide_possible_values = true)]
        key: String,
        /// Taken as typed, even when it starts with '-'
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Remove KEY from config.toml, so the global value or the default applies again
    Unset {
        #[arg(value_parser = ConfigKeyParser, hide_possible_values = true)]
        key: String,
    },
    /// Where the settings file is, whether or not it exists
    Path,
}

/// The providers this build registers, as completion offers them (§13.7).
pub(crate) const COMPLETED_PROVIDERS: &[&str] = &[CLAUDE_CODE];

/// A `config` command's KEY. Any UTF-8 string parses, so a key the registry does not know reaches
/// the engine's `invalid-input` (§6.4) rather than a usage error, while completion offers every
/// registry key and its `provider.<id>.` spelling (§13.7, Decision 14).
#[derive(Clone)]
pub struct ConfigKeyParser;

impl TypedValueParser for ConfigKeyParser {
    type Value = String;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<String, clap::Error> {
        StringValueParser::new().parse_ref(cmd, arg, value)
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        let bare = KEYS.iter().map(|k| k.name.to_owned());
        let prefixed = COMPLETED_PROVIDERS.iter().flat_map(|provider| {
            KEYS.iter()
                .filter(|k| k.per_provider)
                .map(move |k| format!("provider.{provider}.{}", k.name))
        });
        Some(Box::new(bare.chain(prefixed).map(PossibleValue::new)))
    }
}
