use clap::{Parser, Subcommand};

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
    /// The agent CLI to act on (default: claude-code)
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
    /// One line for Claude Code's status bar
    Statusline {
        /// Print the settings.json snippet that sets it up
        #[arg(long = "print-config")]
        print_config: bool,
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
