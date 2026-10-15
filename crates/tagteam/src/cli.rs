use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::PathBuf;

use clap::builder::{PossibleValue, StringValueParser, TypedValueParser};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::aot::{Shell, generate};
use tagteam_core::CLAUDE_CODE;
use tagteam_engine::settings::KEYS;
use tagteam_engine::tokens::TokenPeriod;

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
    #[arg(
        short = 'p',
        long,
        global = true,
        value_name = "PROVIDER",
        value_parser = ProviderParser,
        hide_possible_values = true
    )]
    pub provider: Option<String>,
    /// The account the reference must resolve to (the TUI's guard, §10.4); hidden.
    #[arg(long = "expect-account", global = true, hide = true, value_name = "ID")]
    pub expect_account: Option<String>,
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
        #[arg(
            long = "include-api-key-accounts",
            value_name = "BOOL",
            value_parser = BoolWordParser,
            hide_possible_values = true
        )]
        include_api_key_accounts: Option<String>,
    },
    /// Run the daemon in the foreground: collect usage and switch automatically, until stopped
    ///
    /// One job per provider (or only --provider's). While daemon.autoswitch is true it switches
    /// as `tagteam auto` does, and otherwise it only collects usage, so `tagteam config set
    /// daemon.autoswitch false` pauses it. Notifications follow the notify.* settings. --json
    /// prints one event per line: {"schemaVersion":1,"event":<kind>,"ts":"…Z", …}. It exits 0
    /// when a signal stops it, and 75 when a newer tagteam must take over.
    Daemon {
        #[command(subcommand)]
        action: Option<DaemonAction>,
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
    /// Token use and API cost per account, from Claude Code's transcripts
    ///
    /// Reads new transcript lines into tagteam's index first (not with --no-refresh), then
    /// reports by day (the default), month, session or 5-hour block, a row per account unless
    /// --by says otherwise. With no ACCOUNT, every account shows, and the usage no account can
    /// be credited with as unattributed. The cost is the API list price of the tokens, not a
    /// charge. --json prints {schemaVersion, provider, period, grouping, timeZone, since,
    /// until, rows: [{key, period, account?, model?, accounts, models, tokens: {input, output,
    /// cacheWrite5m, cacheWrite1h, cacheWrite, cacheRead, total, webSearch}, responses,
    /// unfinalized, costUsd, costPartial}], total, index, unpricedModels, prices: {asOf,
    /// overridden}}, with times in ISO 8601 UTC and dates in timeZone.
    #[command(args_conflicts_with_subcommands = true)]
    Tokens {
        // Boxed: a period carries its own arguments, which would double every `Command`.
        #[command(subcommand)]
        period: Option<Box<TokensPeriod>>,
        #[command(flatten)]
        args: TokensArgs,
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
    /// Delete tagteam's data: every account (or only --provider's), never a live login
    ///
    /// Asks first on a terminal; elsewhere, and with --json, it needs --yes. --json prints
    /// {schemaVersion, ok, provider, accounts: [{number, id, email}], displaced, rescues,
    /// storeEmptied, failures: [{what, message}]}. Exits 1 if anything could not be deleted.
    Purge {
        /// Delete without asking
        #[arg(long)]
        yes: bool,
        /// Also delete every `tagteam` Keychain item no account names, for every tagteam data
        /// directory on this Mac (not with --provider)
        #[arg(long = "keychain-orphans")]
        keychain_orphans: bool,
    },
    /// Write the stored accounts to FILE, or `-` for stdout, age-encrypted by default
    ///
    /// An OAuth export hands its logins over rather than copying them: whichever machine
    /// refreshes one first invalidates every other copy. To use an account on two machines,
    /// log in on each. Without --recipient or --recipient-file, a passphrase is asked for on
    /// the terminal. --json prints {schemaVersion, ok, file, encrypted, accounts: [{provider,
    /// number, email, source, inUse}], skipped: [{provider, number, email, reason}]}.
    Export {
        /// Where to write the export; `-`, the default, is stdout
        file: Option<String>,
        /// Export only this account (repeatable)
        #[arg(long)]
        account: Vec<String>,
        /// Keep the whole credential, this machine's keys included
        #[arg(long)]
        full: bool,
        /// Encrypt to this age1… or ssh-ed25519 public key (repeatable)
        #[arg(long, conflicts_with = "plaintext")]
        recipient: Vec<String>,
        /// Encrypt to each public key in FILE, one per line (repeatable)
        #[arg(
            long = "recipient-file",
            value_name = "FILE",
            conflicts_with = "plaintext"
        )]
        recipient_file: Vec<PathBuf>,
        /// Write the export unencrypted
        #[arg(long)]
        plaintext: bool,
    },
    /// Add the accounts of an export (tagteam's, or cswap's version 1) from FILE, or `-` for stdin
    ///
    /// An account already stored is skipped unless --force, or unless it is quarantined. --json
    /// prints {schemaVersion, ok, accounts: [{provider, number, email, outcome, message}],
    /// warnings}. It exits 1 if any account failed.
    Import {
        file: String,
        /// Replace accounts already stored
        #[arg(long)]
        force: bool,
        /// A private key the file is encrypted to: an age identity file or an ssh-ed25519 key
        /// (repeatable)
        #[arg(long, value_name = "FILE")]
        identity: Vec<PathBuf>,
    },
    /// Check tagteam's state and how it works with each agent CLI; tagteam changes nothing itself
    ///
    /// Each check reports ok, info, warn or fail, and every problem names its fix. It exits 1
    /// when a check fails. --json prints {schemaVersion, ok, checks: [{id, provider, status,
    /// message, fix}]}, with provider null for tagteam's own checks.
    Doctor {
        /// Also check that each agent's hosts can be reached; no credential is sent
        #[arg(long)]
        online: bool,
    },
    /// Print a completion script for bash, zsh or fish
    Completions { shell: CompletionShell },
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

/// `tokens`' period (token report spec §11.1): a subcommand, so an account alias spelled like
/// one is never taken for it. A bare `tokens` is `tokens day`.
#[derive(Subcommand)]
pub enum TokensPeriod {
    /// A row per calendar day (the default)
    Day(TokensArgs),
    /// A row per calendar month; --since defaults to twelve months
    Month(TokensArgs),
    /// A row per session, with its first and last times and its project
    Session(TokensArgs),
    /// A row per 5-hour block of each account
    Block(TokensArgs),
}

impl TokensPeriod {
    /// The period and arguments `tokens` asked for (token report spec §11.1). A bare `tokens`
    /// is `day`. With a flag before it, clap takes the period word for the first ACCOUNT
    /// (`args_conflicts_with_subcommands`), so a first ACCOUNT that is exactly a period's name
    /// is that period, and the rest stay ACCOUNTs. Only the first is read so: `tokens alice
    /// month` names two accounts.
    pub fn effective(
        period: Option<Box<TokensPeriod>>,
        mut args: TokensArgs,
    ) -> (TokenPeriod, TokensArgs) {
        match period.map(|p| *p) {
            Some(TokensPeriod::Day(a)) => (TokenPeriod::Day, a),
            Some(TokensPeriod::Month(a)) => (TokenPeriod::Month, a),
            Some(TokensPeriod::Session(a)) => (TokenPeriod::Session, a),
            Some(TokensPeriod::Block(a)) => (TokenPeriod::Block, a),
            None => {
                let named = match args.accounts.first().map(String::as_str) {
                    Some("day") => Some(TokenPeriod::Day),
                    Some("month") => Some(TokenPeriod::Month),
                    Some("session") => Some(TokenPeriod::Session),
                    Some("block") => Some(TokenPeriod::Block),
                    _ => None,
                };
                match named {
                    Some(period) => {
                        args.accounts.remove(0);
                        (period, args)
                    }
                    None => (TokenPeriod::Day, args),
                }
            }
        }
    }
}

/// `tokens`' arguments (token report spec §11.1), the same under every period.
#[derive(Args)]
pub struct TokensArgs {
    /// Only these accounts: a position, alias or email (default: every account, and the
    /// unattributed usage)
    #[arg(value_name = "ACCOUNT")]
    pub accounts: Vec<String>,
    /// Also the usage no account can be credited with
    #[arg(long)]
    pub unattributed: bool,
    /// Only models whose name contains M, in any case (repeatable)
    #[arg(long, value_name = "M")]
    pub model: Vec<String>,
    /// From: a span back like 30d, 12h or 90m, a date YYYY-MM-DD, or an ISO 8601 time
    /// (default: 30d; for month, twelve calendar months)
    #[arg(long, value_name = "WHEN")]
    pub since: Option<String>,
    /// Until, not included: as --since takes it (default: now)
    #[arg(long, value_name = "WHEN")]
    pub until: Option<String>,
    /// A row per account, model, account,model or none (default: account)
    #[arg(
        long,
        value_name = "GROUPS",
        value_parser = GroupingParser,
        hide_possible_values = true
    )]
    pub by: Option<String>,
    /// Days and months in this IANA time zone, or UTC (default: the system's)
    #[arg(long, value_name = "ZONE")]
    pub tz: Option<String>,
    /// Print the rows as CSV
    #[arg(long)]
    pub csv: bool,
    /// Report from the index as it is, reading no transcript
    #[arg(long = "no-refresh")]
    pub no_refresh: bool,
}

/// `tokens --by`. Any UTF-8 string parses, so the command refuses a bad value with its own
/// usage error (token report spec §14), while completion offers the values; `account,model`
/// is hidden from it, since a completion word holds no comma.
#[derive(Clone)]
pub struct GroupingParser;

impl TypedValueParser for GroupingParser {
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
        Some(Box::new(
            [
                PossibleValue::new("account"),
                PossibleValue::new("model"),
                PossibleValue::new("account,model").hide(true),
                PossibleValue::new("none"),
            ]
            .into_iter(),
        ))
    }
}

impl Command {
    /// Whether the command reads or writes a Keychain item on macOS, and so runs the lock check
    /// first (Appendix A.3). `list` and `status` are not among them although they collect usage
    /// and may read Keychain items: by a plan ruling they degrade to `keychain_unavailable`
    /// rows rather than run the lock check, so a script gets rows, not a `keychain-locked`
    /// failure. `history` and `statusline` read only the store and `~/.claude.json`, `tokens`
    /// only the store and Claude Code's transcripts, and the rest need no Keychain item. A
    /// recovery under their mutation lock (Task 21) only reads the Keychain, tri-state, and
    /// leaves what it cannot decide to the next command that checks.
    /// `auto` checks before its first tick, a dry run too (§11.1): every tick reads its
    /// accounts' items, and a real one switches.
    /// `run` checks for itself, and only once it knows it launches a session: plain `claude`
    /// touches no item, so an unmapped directory never waits on an unlock prompt (§12.7).
    /// `purge` deletes its accounts' vault items, and with `--keychain-orphans` every `tagteam`
    /// item, so it checks too, after its run shell's refusal (§10.5).
    /// `export` and `import` check for themselves, once their destination, keys and file are
    /// settled (§13.3): a bad argument is refused first, whatever the Keychain's state.
    pub fn touches_keychain(&self) -> bool {
        matches!(
            self,
            Command::Add { .. }
                | Command::AddToken { .. }
                | Command::Switch { .. }
                | Command::Remove { .. }
                | Command::Auto { .. }
                | Command::Purge { .. }
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

/// `tagteam daemon`'s service commands (D8, D9). Without one, `tagteam daemon` runs the daemon
/// in the foreground.
#[derive(Subcommand)]
pub enum DaemonAction {
    /// Install the service that runs the daemon at login, and start it
    ///
    /// macOS: a launchd agent in ~/Library/LaunchAgents. Linux: a systemd user unit. The unit
    /// records this tagteam's path, and the variables that place its store, Claude Code's
    /// home, a proxy and the log. Running it again replaces the unit and restarts the daemon.
    /// --json prints {schemaVersion, ok, service: {kind, path, label}, started, warnings}.
    Install {
        /// Print the unit for this platform and change nothing
        #[arg(long, conflicts_with = "no_start")]
        print: bool,
        /// Write the unit and enable it for the next login, starting and stopping nothing
        #[arg(long = "no-start")]
        no_start: bool,
    },
    /// Stop the daemon and remove its service
    ///
    /// --json prints {schemaVersion, ok, removed, stopped}; removed is false when none was
    /// installed, and stopped is true when the unit's file was gone but its service was stopped.
    Uninstall,
    /// Whether the daemon runs: its own status from its socket, else its lock and its service
    ///
    /// Exits 0 when it runs, 3 when it does not, and 1 on an error. --json prints
    /// {schemaVersion, running, pid, daemon, service: {kind, path, installed, loaded,
    /// lastExitStatus}}, daemon being the daemon's own status, or null.
    Status,
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

/// `--provider`. Any UTF-8 string parses, so a provider this build lacks still reaches the
/// engine's `unknown-provider`, while completion offers the providers it has (§13.7).
#[derive(Clone)]
pub struct ProviderParser;

impl TypedValueParser for ProviderParser {
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
        Some(Box::new(
            COMPLETED_PROVIDERS.iter().map(|p| PossibleValue::new(*p)),
        ))
    }
}

/// The words `--include-api-key-accounts` takes (`parse_bool`'s closed set).
const BOOL_WORDS: &[&str] = &["true", "false", "1", "0", "yes", "no"];

/// `--include-api-key-accounts`. Any UTF-8 string parses, so the command still refuses a bad
/// word with its own message, while completion offers the six it takes (§13.7).
#[derive(Clone)]
pub struct BoolWordParser;

impl TypedValueParser for BoolWordParser {
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
        Some(Box::new(BOOL_WORDS.iter().map(|w| PossibleValue::new(*w))))
    }
}

/// The shells `tagteam completions` writes a script for (§13.7).
#[derive(Clone, Copy, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

impl CompletionShell {
    /// The script, generated by `clap_complete` from these definitions (§13.7). It is static:
    /// commands, flags, provider IDs, settings keys and the other fixed values complete, and
    /// account references do not. It is built in memory, so a closed stdout is the caller's
    /// write error rather than a panic inside the generator.
    pub fn script(self) -> Vec<u8> {
        let shell = match self {
            CompletionShell::Bash => Shell::Bash,
            CompletionShell::Zsh => Shell::Zsh,
            CompletionShell::Fish => Shell::Fish,
        };
        let mut cmd = Cli::command();
        let mut script = Vec::new();
        generate(shell, &mut cmd, "tagteam", &mut script);
        let mut script = without_hidden_flags(script, &cmd);
        if let CompletionShell::Fish = self {
            fish_positionals(&cmd, &mut script);
        }
        script
    }
}

/// `script` without the global flags `cmd` hides (`--expect-account`). clap keeps a hidden flag
/// out of `--help`, but clap_complete's bash, zsh and fish writers still list it, so this
/// removes, line by line, what each writes for it:
/// - bash: the flag's word in each `opts=` list, and its `case "${prev}"` arm;
/// - zsh: its `'--flag=…' \` line in each `_arguments` call;
/// - fish: its `complete … -l flag …` lines, and its word in the global option spec.
fn without_hidden_flags(script: Vec<u8>, cmd: &clap::Command) -> Vec<u8> {
    let hidden: Vec<&str> = cmd
        .get_arguments()
        .filter(|a| a.is_hide_set() && a.is_global_set())
        .filter_map(|a| a.get_long())
        .collect();
    let text = String::from_utf8(script).expect("clap_complete writes UTF-8");
    let mut out = String::with_capacity(text.len());
    let mut in_arm = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if in_arm {
            in_arm = trimmed != ";;";
            continue;
        }
        let mut line = line.to_owned();
        let mut drop = false;
        for flag in &hidden {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            if trimmed == format!("--{flag})") {
                in_arm = true;
                drop = true;
            }
            drop |= trimmed.starts_with(&format!("'--{flag}"));
            drop |= trimmed.starts_with("complete ") && words.windows(2).any(|w| w == ["-l", flag]);
            line = line
                .replace(&format!(" --{flag}"), "")
                .replace(&format!(" {flag}="), "");
        }
        if !drop {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.into_bytes()
}

/// clap_complete's fish script completes options and subcommands, but no positional argument's
/// values, so `config get <KEY>` and `completions <SHELL>` would offer nothing. This appends,
/// from the same definitions, a `complete` line for each positional with fixed values, and the
/// helper their conditions read. `__fish_tagteam_words` lists the words before the cursor that
/// are neither an option nor the word after an option that takes a value, global ones included.
/// A line applies only while those words are exactly the subcommands that lead to its
/// positional, then one word for each positional before it, so it offers nothing once its
/// positional is given.
fn fish_positionals(cmd: &clap::Command, script: &mut Vec<u8>) {
    let mut valued = BTreeSet::new();
    options_taking_values(cmd, &mut valued);
    let valued = valued.into_iter().collect::<Vec<_>>().join(" ");
    let _ = write!(
        script,
        r#"
# The words before the cursor that are neither an option nor an option's value: the
# subcommands given, then the positional arguments given.
function __fish_tagteam_words
    set -l words (commandline -opc)
    set -e words[1]
    set -l skip 0
    for word in $words
        if test $skip = 1
            set skip 0
        else if contains -- $word {valued}
            set skip 1
        else if not string match -q -- '-?*' $word
            echo $word
        end
    end
    # An option still waiting for its value: the word being completed is that value, so an
    # empty word makes every line's pattern fail.
    if test $skip = 1
        echo ''
    end
end

"#
    );
    fish_positional_lines(cmd, &mut Vec::new(), script);
    fish_global_option_values(cmd, script);
}

/// clap_complete's `__fish_tagteam_needs_command` fails while an option still waits for its
/// value, so a global option's values (`tagteam --provider <TAB>`) would complete as files before
/// a subcommand. This adds, for each global option with fixed values, a helper that holds when
/// the word before the cursor is one of its spellings, and a line completing the values there.
fn fish_global_option_values(cmd: &clap::Command, script: &mut Vec<u8>) {
    for arg in cmd
        .get_arguments()
        .filter(|a| a.is_global_set() && !a.is_positional() && a.get_action().takes_values())
    {
        let values: Vec<String> = arg
            .get_possible_values()
            .into_iter()
            .filter(|v| !v.is_hide_set())
            .map(|v| v.get_name().to_owned())
            .collect();
        if values.is_empty() {
            continue;
        }
        let shorts = arg.get_short_and_visible_aliases().unwrap_or_default();
        let longs = arg.get_long_and_visible_aliases().unwrap_or_default();
        let spellings: Vec<String> = shorts
            .into_iter()
            .map(|s| format!("-{s}"))
            .chain(longs.into_iter().map(|l| format!("--{l}")))
            .collect();
        let name = arg.get_id().as_str().replace('-', "_");
        let _ = write!(
            script,
            r#"
function __fish_tagteam_after_{name}
    set -l words (commandline -opc)
    contains -- $words[-1] {}
end
complete -c tagteam -n "__fish_tagteam_after_{name}" -f -a "{}"
"#,
            spellings.join(" "),
            values.join(" ")
        );
    }
}

/// Every spelling of an option under `cmd` that takes a value, such as `-p` and `--provider`.
fn options_taking_values(cmd: &clap::Command, valued: &mut BTreeSet<String>) {
    for arg in cmd
        .get_arguments()
        .filter(|a| !a.is_positional() && !a.is_hide_set() && a.get_action().takes_values())
    {
        let shorts = arg.get_short_and_visible_aliases().unwrap_or_default();
        let longs = arg.get_long_and_visible_aliases().unwrap_or_default();
        valued.extend(shorts.into_iter().map(|s| format!("-{s}")));
        valued.extend(longs.into_iter().map(|l| format!("--{l}")));
    }
    for sub in cmd.get_subcommands() {
        options_taking_values(sub, valued);
    }
}

/// A `complete` line for each positional under `cmd` with fixed values. `path` holds the
/// patterns of the subcommands that lead there: a name, or `(name|alias)`.
fn fish_positional_lines(cmd: &clap::Command, path: &mut Vec<String>, script: &mut Vec<u8>) {
    for sub in cmd.get_subcommands() {
        let names = sub.get_name_and_visible_aliases();
        path.push(match names.as_slice() {
            [name] => (*name).to_owned(),
            _ => format!("({})", names.join("|")),
        });
        for (before, arg) in sub.get_positionals().enumerate() {
            let values: Vec<String> = arg
                .get_possible_values()
                .into_iter()
                .filter(|v| !v.is_hide_set())
                .map(|v| v.get_name().to_owned())
                .collect();
            if values.is_empty() {
                continue;
            }
            let words: Vec<&str> = path
                .iter()
                .map(String::as_str)
                .chain(std::iter::repeat_n(r"\S+", before))
                .collect();
            let condition = format!(
                r"__fish_tagteam_words | string join ' ' | string match -qr '^{}\$'",
                words.join(" ")
            );
            let _ = writeln!(
                script,
                "complete -c tagteam -n \"{condition}\" -f -a \"{}\"",
                values.join(" ")
            );
        }
        fish_positional_lines(sub, path, script);
        path.pop();
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use tagteam_engine::settings::parse_bool;

    use tagteam_engine::tokens::TokenPeriod;

    use super::{BOOL_WORDS, Cli, Command, TokensPeriod};

    /// Every string of `a-z0-9`, up to `max` characters.
    fn words_up_to(max: usize) -> Vec<String> {
        let mut all = Vec::new();
        let mut layer = vec![String::new()];
        for _ in 0..max {
            layer = layer
                .iter()
                .flat_map(|w| ('a'..='z').chain('0'..='9').map(move |c| format!("{w}{c}")))
                .collect();
            all.extend(layer.iter().cloned());
        }
        all
    }

    #[test]
    fn the_completed_boolean_words_are_exactly_the_ones_parse_bool_takes() {
        // §6.4's six. `BOOL_WORDS` restates `parse_bool`'s set for completion, so neither may
        // drift: each word completes to a value that parses, and nothing else lowercase does.
        assert_eq!(BOOL_WORDS, ["true", "false", "1", "0", "yes", "no"]);
        for word in BOOL_WORDS {
            assert!(
                parse_bool(word).is_some(),
                "{word:?} completes but is refused"
            );
        }
        let alike = [
            "on", "off", "y", "n", "t", "f", "enable", "disable", "enabled", "yeah",
        ];
        for word in words_up_to(4).iter().map(String::as_str).chain(alike) {
            assert!(
                parse_bool(word).is_none() || BOOL_WORDS.contains(&word),
                "{word:?} is accepted but never completed"
            );
        }
    }

    /// `tagteam <args>` as `tokens`: the period's name (none when bare), and the ACCOUNTs and
    /// `--by` of whichever level took them.
    fn tokens(args: &[&str]) -> (Option<&'static str>, Vec<String>, Option<String>) {
        let cli = Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied()))
            .unwrap_or_else(|e| panic!("{args:?}: {e}"));
        let Some(Command::Tokens { period, args }) = cli.command else {
            panic!("not tokens");
        };
        let (name, args) = match period.map(|p| *p) {
            None => (None, args),
            Some(TokensPeriod::Day(a)) => (Some("day"), a),
            Some(TokensPeriod::Month(a)) => (Some("month"), a),
            Some(TokensPeriod::Session(a)) => (Some("session"), a),
            Some(TokensPeriod::Block(a)) => (Some("block"), a),
        };
        (name, args.accounts, args.by)
    }

    #[test]
    fn tokens_takes_its_period_as_a_subcommand_and_is_day_when_bare() {
        // Token report spec §11.1: the period is a subcommand, so an ACCOUNT spelled like one
        // follows a period; a bare `tokens` takes the same arguments, as `day`.
        let owned = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(tokens(&["tokens"]), (None, vec![], None));
        assert_eq!(
            tokens(&["tokens", "2", "--by", "model"]),
            (None, owned(&["2"]), Some("model".to_owned()))
        );
        assert_eq!(
            tokens(&["tokens", "month", "work", "1", "--by", "account,model"]),
            (
                Some("month"),
                owned(&["work", "1"]),
                Some("account,model".to_owned())
            )
        );
        assert_eq!(
            tokens(&["tokens", "block", "day"]),
            (Some("block"), owned(&["day"]), None),
            "an account named like a period, after one"
        );
        assert_eq!(tokens(&["tokens", "session"]).0, Some("session"));
    }

    /// The period and ACCOUNTs `tagteam <args>` asks `tokens` for, once normalized.
    fn effective(args: &[&str]) -> (TokenPeriod, Vec<String>) {
        let cli = Cli::try_parse_from(std::iter::once("tagteam").chain(args.iter().copied()))
            .unwrap_or_else(|e| panic!("{args:?}: {e}"));
        let Some(Command::Tokens { period, args }) = cli.command else {
            panic!("not tokens");
        };
        let (period, args) = TokensPeriod::effective(period, args);
        (period, args.accounts)
    }

    #[test]
    fn a_flag_before_the_period_leaves_it_the_period() {
        // With a flag first clap reads the period word as an ACCOUNT; the first ACCOUNT that
        // is exactly a period's name is that period (spec §11.1), and only the first.
        let owned = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(effective(&["tokens"]), (TokenPeriod::Day, vec![]));
        assert_eq!(
            effective(&["tokens", "--json", "month"]),
            (TokenPeriod::Month, vec![])
        );
        assert_eq!(
            effective(&["tokens", "--tz", "UTC", "session", "alice"]),
            (TokenPeriod::Session, owned(&["alice"]))
        );
        assert_eq!(
            effective(&["tokens", "alice", "month"]),
            (TokenPeriod::Day, owned(&["alice", "month"])),
            "only the first ACCOUNT can be the period"
        );
        assert_eq!(
            effective(&["tokens", "month", "--json", "block"]),
            (TokenPeriod::Month, owned(&["block"])),
            "a period that matched keeps the rest as ACCOUNTs"
        );
        assert_eq!(
            effective(&["tokens", "--json", "block", "day"]),
            (TokenPeriod::Block, owned(&["day"]))
        );
    }

    #[test]
    fn daemon_alone_is_the_foreground_run_and_its_service_commands_parse() {
        use clap::Parser;

        use super::{Cli, Command, DaemonAction};
        let parse = |args: &[&str]| Cli::try_parse_from(args).map(|c| c.command);
        assert!(matches!(
            parse(&["tagteam", "daemon"]),
            Ok(Some(Command::Daemon { action: None }))
        ));
        assert!(matches!(
            parse(&["tagteam", "daemon", "install"]),
            Ok(Some(Command::Daemon {
                action: Some(DaemonAction::Install {
                    print: false,
                    no_start: false
                })
            }))
        ));
        assert!(matches!(
            parse(&["tagteam", "daemon", "install", "--no-start"]),
            Ok(Some(Command::Daemon {
                action: Some(DaemonAction::Install {
                    print: false,
                    no_start: true
                })
            }))
        ));
        assert!(matches!(
            parse(&["tagteam", "daemon", "install", "--print"]),
            Ok(Some(Command::Daemon {
                action: Some(DaemonAction::Install {
                    print: true,
                    no_start: false
                })
            }))
        ));
        assert!(
            parse(&["tagteam", "daemon", "install", "--print", "--no-start"]).is_err(),
            "--print changes nothing, so --no-start means nothing beside it"
        );
        assert!(matches!(
            parse(&["tagteam", "daemon", "uninstall"]),
            Ok(Some(Command::Daemon {
                action: Some(DaemonAction::Uninstall)
            }))
        ));
        let cli = Cli::try_parse_from(["tagteam", "daemon", "status", "--json"]).unwrap();
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Some(Command::Daemon {
                action: Some(DaemonAction::Status)
            })
        ));
    }
}
