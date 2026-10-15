# tagteam

[![CI](https://github.com/michaelrigart/tagteam/actions/workflows/ci.yml/badge.svg)](https://github.com/michaelrigart/tagteam/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/michaelrigart/tagteam)](https://github.com/michaelrigart/tagteam/releases)
[![License: MIT](https://img.shields.io/github/license/michaelrigart/tagteam)](LICENSE)

tagteam is a multi-account switcher for [Claude Code](https://www.anthropic.com/claude-code).
It stores several Claude Code logins, switches the active one, runs accounts side by side, and
can switch automatically before an account hits its rate limit.

## Why

People who use more than one Claude Code account (personal and work, or several subscriptions)
usually log out and in again to change accounts. Doing that by hand is slow, and it is easy to
get wrong.

tagteam changes only the account identity. Memory, instruction files, history, projects,
settings, skills, agents, commands, plugins and MCP configuration are never touched by a switch.
The exact list of what tagteam may write to Claude Code's state is in the
[design spec](docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md) (section 3), and a
test pins it.

- **Switch** between stored logins with one command, by position, alias or email.
- **See usage** for every account: the 5-hour and 7-day windows, spend, per-model limits, and
  when each one resets. `tagteam history` shows the burn rate and when a window runs out.
- **Count tokens per account.** `tagteam tokens` reports token use and its API-equivalent
  cost from Claude Code's transcripts, by day, month, session or 5-hour block, and which account
  each request went to.
- **Run accounts side by side.** `tagteam run` starts Claude Code as another account in its own
  session, while your default login stays as it is. Sessions share your history, memory and
  settings.
- **Switch automatically.** `tagteam auto` moves to a better account before the current one
  reaches a threshold. The daemon does the same in the background, keeps every account's usage
  fresh, and tells you when it switches.
- **Keep refreshed tokens safe.** Every refresh-token successor tagteam receives is persisted:
  to the vault, to a rescue file if that write fails, or under `displaced/` if it belongs to
  another account. If every write fails, tagteam reports it.
- **One binary** for macOS and Linux (statically linked on Linux), with no runtime beyond
  `claude` itself (and `/usr/bin/security` on macOS).

tagteam began as a Rust rewrite of [claude-swap](https://github.com/realiti4/claude-swap) and
keeps its JSON field names, so scripts written for claude-swap can read tagteam's output. See
[NOTICE](NOTICE).

> **Status:** versions are dates, `YYYYMMDD.HHMM.0` in UTC, so a version number says when a
> release was made, not whether it breaks anything. Read the [CHANGELOG](CHANGELOG.md) before
> upgrading: the interface and the on-disk format can still change between releases. tagteam
> relies on Claude Code internals that Anthropic does not document or guarantee, so a Claude
> Code update can break it. See [Requirements](#requirements).

## Requirements

- **macOS** (Apple silicon or Intel) or **Linux** (x86-64 or aarch64). Windows is not supported.
- **Claude Code**, with `claude` on your `PATH`. tagteam was developed against Claude Code
  2.1.283 and later, and the newest version it has been checked against is **2.1.292**
  (`crates/tagteam-cc/compat/tested-cc-version`). It does not refuse a newer version, but
  `tagteam doctor` warns about one. Older versions are untested.
- tagteam refuses to run as root. The one exception is a Linux container whose root
  filesystem is an overlay mount, such as Docker's.

## Install

### Homebrew

Available once the first release is published:

```sh
brew install michaelrigart/tap/tagteam
```

### crates.io

Available once the first release is published:

```sh
cargo install tagteam --locked
```

You need Rust 1.85 or newer (the minimum supported version, `rust-version` in `Cargo.toml`)
and a C compiler, because tagteam bundles SQLite. Only `tagteam` is meant to be installed; the
`tagteam-*` crates it depends on are internals with no stable API.

### From source

Build the current `main` instead of a release. You need [rustup](https://rustup.rs) and a C
compiler (tagteam bundles SQLite). The repository's `rust-toolchain.toml` pins Rust 1.88.0, which
rustup installs on first use.

```sh
git clone https://github.com/michaelrigart/tagteam.git
cd tagteam
cargo install --path crates/tagteam --locked
```

This puts `tagteam` in `~/.cargo/bin`. To build without installing, run
`cargo build --release -p tagteam`; the binary is `target/release/tagteam`.

Shell completions for bash, zsh and fish are printed by `tagteam completions <shell>`.

## Quick start

Log in to Claude Code as usual, then store that login:

```sh
claude                  # log in as your first account
tagteam add             # store the live login
```

Log in as your second account in Claude Code (for example with `/login`), and store that one
too:

```sh
tagteam add --alias work
```

Then:

```sh
tagteam list            # accounts with usage and reset times
tagteam status          # the live account
tagteam switch          # switch to the next account
tagteam switch work     # switch to an account by position, alias or email
tagteam switch --strategy best   # switch to the account with the most headroom
```

A running Claude Code session picks up the new login within about 30 seconds on macOS, and on
the next message on Linux. If an API-key account is on either side of the switch, restart
Claude Code.

For an API key (`sk-ant-api...`) or a setup token, use `tagteam add-token`. Pass `-` to read the
token from stdin so it stays out of your shell history.

## Commands

Run `tagteam <command> --help` for the full options. Every command accepts `--provider`/`-p`
(Claude Code is the only provider at present). Most commands accept `--json` and then print
one JSON object on stdout. The exceptions: `auto --json` and `daemon --json` print one event
per line,
`run --json` covers only errors before Claude Code starts, and `statusline`, `shell-init`
and `completions` refuse `--json`. A bare `tagteam` runs `list`.

| Command | What it does |
|---|---|
| `list` (`ls`), `status` | Show the stored accounts with usage, or only the live account |
| `add`, `add-token` | Store the current Claude Code login, or an API key or setup token |
| `remove` (`rm`), `disable`, `enable` | Delete an account (never the live login), or hold it out of automatic selection |
| `alias`, `move` | Name an account, or change its position |
| `switch` | Switch to the next account, to a named one, or to the one a strategy picks (`best`, `next-available`) |
| `auto` | Switch automatically before a rate limit, until stopped. `--once` ticks once; `--dry-run` decides without switching |
| `daemon` | Switch automatically and keep usage fresh in the background. `install` and `uninstall` its login service, show its `status`, or run it in the foreground |
| `run`, `map`, `unmap`, `shell-init` | Run Claude Code as an account in a separate session, and map directories to accounts |
| `history` | Usage history, burn rate, and when each window runs out |
| `tokens` | Token use and API cost per account and model, by day, month, session or 5-hour block, from Claude Code's transcripts |
| `statusline` | One line for Claude Code's status bar |
| `export`, `import` | Move accounts between machines (age-encrypted by default); `import` also reads claude-swap v1 exports |
| `config` | Read and edit `config.toml` (`list`, `get`, `set`, `unset`, `path`) |
| `doctor` | Check tagteam's state and its interop with Claude Code |
| `displaced` | List, or delete, credentials tagteam set aside rather than overwrite |
| `purge` | Delete tagteam's data (see [Uninstall](#uninstall-and-purge)) |
| `completions` | Print a completion script for bash, zsh or fish |

Exit codes: `0` success, `1` error, `2` usage error, and 128 plus the signal number when
interrupted (`130` for Ctrl-C). `tagteam auto --once` uses `0` switched, `1` error, `2` no
action and `3` blocked. `tagteam daemon` exits `0` when a signal stops it and `75` when it
must be restarted on a new version. `tagteam daemon status` exits `0` when the daemon runs
and `3` when it does not.

### Automatic switching

```sh
tagteam auto
```

`auto` runs in the foreground until you press Ctrl-C and prints a line per check. It needs at
least two switchable accounts. By default it switches away above 90% usage, checks every 60
seconds and waits 300 seconds between automatic switches. Change these with flags
(`--threshold`, `--interval`, `--cooldown`, `--strategy`) or with the settings
`autoswitch.threshold`, `autoswitch.interval_seconds`, `autoswitch.cooldown_seconds` and
`autoswitch.strategy`, for example `tagteam config set autoswitch.threshold 85`. To switch in
the background, without a terminal, use the [daemon](#daemon), which reads the same settings.

### Daemon

The daemon runs tagteam in the background. Install it once:

```sh
tagteam daemon install
```

From then on it starts when you log in, and restarts if it fails. While it runs, it:

- keeps every account's usage fresh, on the same schedule and within the same request budget
  as the commands, so `list` rarely has to fetch and `statusline` shows fresh numbers;
- switches accounts automatically, exactly as `tagteam auto` does, for every provider with at
  least two switchable accounts;
- tells you about the events that matter (see below);
- serves a local socket that the TUI and scripts can read.

The daemon switches accounts unattended, and `install` says so. To pause automatic switching
and keep only the usage collection, change a setting. It takes effect within a few seconds,
and lasts until you change it back:

```sh
tagteam config set daemon.autoswitch false   # pause
tagteam config set daemon.autoswitch true    # resume
```

While the daemon switches a provider, `tagteam auto` refuses to run for it and says so. Pause
the daemon first if you want `auto` in a terminal.

`tagteam daemon status` shows whether the daemon runs, what it is doing for each provider, and
the state of its service. `tagteam daemon uninstall` stops it and removes the service.

**Notifications.** The daemon tells you when it switches accounts, when an account needs a new
login, and when the live account reaches the switch threshold and the daemon does not switch
away from it:

```
Switched 1 · work → 2 · spare (5h at 92%)
```

On macOS they come from `osascript` and show as Script Editor, which asks once for permission
to send notifications. On Linux they need `notify-send`. Turn them off with
`tagteam config set notify.enabled false`, or choose which ones you get with `notify.events`,
for example `tagteam config set notify.events switch,quarantine`.

**The service.** On macOS it is a LaunchAgent,
`~/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist`, which needs you to be logged
in to the desktop; over SSH alone, run `tagteam daemon` in the foreground instead. On Linux it
is a systemd user unit, `~/.config/systemd/user/tagteam.service`. To keep it running after you
log out, run `loginctl enable-linger`. The unit records the variables that decide which store,
Claude Code home, proxy and log the daemon uses (`XDG_*`, `CLAUDE_CONFIG_DIR`,
`CLAUDE_SECURESTORAGE_CONFIG_DIR`, the proxy variables, `TAGTEAM_LOG` and `PATH`), so run `tagteam daemon install` again after you change
them. `install --print` prints the unit without installing anything, and `--no-start` installs
it for your next login without starting it now.

If the macOS login keychain is locked, the daemon waits for it, trying again after a minute and
then less and less often, up to every 15 minutes. When you upgrade tagteam, the daemon notices
within a minute, exits with code 75, and the service starts the new version. `tagteam daemon`
runs it in the foreground, with a line per check as `auto` prints, for use under a process
supervisor of your own.

### Running accounts side by side

`tagteam run` starts `claude` as another account in its own session, without changing your
default login:

```sh
tagteam run work                 # run Claude Code as the "work" account
tagteam run work -- --resume     # everything after -- goes to claude
```

Each account gets a profile directory under tagteam's data directory, and `tagteam run` points
Claude Code at it with `CLAUDE_CONFIG_DIR`. The profile links a fixed list of entries to your
default home, so sessions share them: history, projects and memory, `CLAUDE.md`, settings,
keybindings, your agents, commands, skills, plugins, hooks, rules, workflows, output styles
and themes, and Claude Code's file history, paste cache, shell snapshots, session environments
and task lists. Any other entry, including the credential, stays private to the profile. To
share more of your own files, add their names to the `run.share_extra` setting (section 12.2
of the [design spec](docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md)).

To pick the account by directory, map it, and add the shell wrapper to your shell rc file:

```sh
tagteam map work ~/Code/work     # `run` below ~/Code/work launches as "work"
eval "$(tagteam shell-init zsh)" # in ~/.zshrc; bash is the same, with bash
tagteam shell-init fish | source # in ~/.config/fish/config.fish
```

With the wrapper, `claude` runs through `tagteam run`. Where no mapping applies, it starts plain
`claude`.

### Statusline

`tagteam statusline` prints one line for Claude Code's status bar, such as
`work · 5h 31% · 7d 12%`. It reads stored data only, with no network and no Keychain access,
so it is fast.

tagteam never edits Claude Code's settings. Print the snippet and paste it into
`~/.claude/settings.json` (or `settings.json` in your `CLAUDE_CONFIG_DIR`):

```sh
tagteam statusline --print-config
```

```json
{
  "statusLine": {
    "type": "command",
    "command": "tagteam statusline"
  }
}
```

`tagteam` must be on the `PATH` that Claude Code sees. Change the layout with
`tagteam config set statusline.format "<format>"`. The placeholders are `{account}`,
`{position}`, `{email}`, `{5h}`, `{7d}`, `{5h_reset}`, `{7d_reset}`, `{spend}`,
`{model:<name>}` and `{stale}`.

## Where data lives and how credentials are handled

tagteam follows the XDG layout on both macOS and Linux, and honours `XDG_*` variables that are
set to absolute paths. Directories are created lazily with mode 0700.

| What | Where |
|---|---|
| Settings | `$XDG_CONFIG_HOME/tagteam/config.toml` (default `~/.config/tagteam/`) |
| Store (account metadata, usage, history, the token index) | `$XDG_DATA_HOME/tagteam/tagteam.db` (default `~/.local/share/tagteam/`) |
| Vault on Linux | `$XDG_DATA_HOME/tagteam/vault/` |
| Session profiles | `$XDG_DATA_HOME/tagteam/sessions/` |
| Rescued and displaced credentials | `$XDG_DATA_HOME/tagteam/rescue/` and `displaced/` |
| Log | `$XDG_STATE_HOME/tagteam/tagteam.log` (default `~/.local/state/tagteam/`) |
| Daemon socket and lock | `$XDG_DATA_HOME/tagteam/daemon.sock` and `locks/daemon.lock` |
| Daemon service unit | macOS: `~/Library/LaunchAgents/io.github.michaelrigart.tagteam.plist`; Linux: `$XDG_CONFIG_HOME/systemd/user/tagteam.service` |

`tagteam config path` prints the settings file's location.

- **Credentials** are held in the vault. On macOS the vault is the login Keychain (service
  `tagteam`, an item per account plus one for its previous generation), accessed only through
  `/usr/bin/security`. On Linux it is `vault/<id>.json` and `<id>.prev.json`, mode 0600 in a
  0700 directory. The store holds metadata only, never a credential.
- **Session profiles** used by `tagteam run` hold their account's credential the way Claude
  Code keeps it. tagteam writes `sessions/<id>/.credentials.json` (mode 0600); on macOS,
  Claude Code moves it into its own Keychain item, `Claude Code-credentials-<hash>`, on its
  first credential write.
- **Rescued and displaced credentials** are plain files with mode 0600 on both platforms,
  because they must not depend on the Keychain. `rescue/` holds a refreshed token whose vault
  write failed. `displaced/` holds a credential tagteam would otherwise have overwritten, such
  as a live login that was not stored. tagteam never reads a displaced credential back;
  `tagteam displaced` lists them.
- **The token index** holds counts, model names, times, session ids and working directories
  read from Claude Code's transcripts, never their content, and response ids only as a hash. It
  keeps 365 days by default (`tokens.retention_days`), about 100 bytes a response: some 150 MB a
  year at a heavy user's rate. It outlives Claude Code's own 30-day transcript cleanup.
- **The log** never contains an email, a token or any part of a credential.
- **The daemon's socket** has mode 0600 and answers only your own user. No credential ever
  crosses it. Its service unit has mode 0600 too, because a proxy URL it records can hold a
  password.
- **Exports** are encrypted with age by default, using a passphrase or recipient keys.
  `--plaintext` is an explicit opt-out. An OAuth export hands a login over rather than copying
  it: whichever machine refreshes the token first invalidates the other copy. To use one
  account on two machines, log in on each.
- **Network.** The only hosts in tagteam's source are Claude Code's own: `platform.claude.com`
  (token refresh) and `api.anthropic.com` (profile and usage). `tagteam doctor --online` only
  checks that these hosts are reachable and sends no credential. `tagteam statusline` makes no
  network request, and neither does `tagteam tokens`: its prices ship with tagteam.

See [SECURITY.md](SECURITY.md) for how to report a vulnerability.

## Uninstall and purge

If you installed the daemon, remove it first: `tagteam daemon uninstall` stops it and deletes
its service unit.

`tagteam purge` deletes everything tagteam stores: the accounts and their vault entries
(including the macOS Keychain items), session profiles, rescued and displaced credentials, the
store's contents and the log. It asks for confirmation on a terminal, and needs `--yes`
elsewhere. It never deletes or replaces the live Claude Code login, so Claude Code keeps working
as the account that is live now.

```sh
tagteam purge                    # asks first; add --yes to skip the prompt
tagteam purge --keychain-orphans # macOS: also delete leftover `tagteam` Keychain items
                                 # of every tagteam data directory on this Mac
```

Purge keeps your `config.toml`, the lock files and the emptied store file. To remove them,
delete the `tagteam` directories under `~/.config`, `~/.local/share` and `~/.local/state` (or
their `XDG_*` equivalents). Also remove the `statusLine` entry from Claude Code's `settings.json` and the
`shell-init` line from your shell rc file if you added them. tagteam never edited those.

Then remove the program:

```sh
brew uninstall tagteam           # Homebrew
cargo uninstall tagteam          # cargo install
```

Run `purge` before you uninstall. Without it, the Keychain items and data files stay behind.

## Contributing

Bug reports, feature requests and pull requests are welcome. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the toolchain, the tests and the design-record
conventions. The design is specified in
[docs/superpowers/specs/](docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md).

## Disclaimer

tagteam is an independent project. It is not affiliated with, endorsed by or sponsored by
Anthropic. Claude and Claude Code are trademarks of Anthropic. tagteam stores logins, refreshes
their tokens and reads usage through Claude Code's own OAuth endpoints. You are responsible for
complying with the terms of service of Anthropic and of any other provider whose accounts you use
with it.

## License

[MIT](LICENSE). tagteam's behaviour is derived from claude-swap (MIT, Onur Cetinkol); see
[NOTICE](NOTICE).
