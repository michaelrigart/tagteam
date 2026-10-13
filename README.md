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
- **Run accounts side by side.** `tagteam run` starts Claude Code as another account in its own
  session, while your default login stays as it is. Sessions share your history, memory and
  settings.
- **Switch automatically.** `tagteam auto` moves to a better account before the current one
  reaches a threshold.
- **Keep refreshed tokens safe.** Every refresh-token successor tagteam receives is persisted:
  to the vault, to a rescue file if that write fails, or under `displaced/` if it belongs to
  another account. If every write fails, tagteam reports it.
- **One binary** for macOS and Linux (statically linked on Linux), with no runtime beyond
  `claude` itself (and `/usr/bin/security` on macOS).

tagteam began as a Rust rewrite of [claude-swap](https://github.com/realiti4/claude-swap) and
keeps its JSON field names, so scripts written for claude-swap can read tagteam's output. See
[NOTICE](NOTICE).

> **Status:** pre-1.0. The interface and the on-disk format can still change between 0.x
> releases. tagteam relies on Claude Code internals that Anthropic does not document or
> guarantee, so a Claude Code update can break it. See [Requirements](#requirements).

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

Available from v0.1.0, once that release is published:

```sh
brew install michaelrigart/tap/tagteam
```

### From source

You need [rustup](https://rustup.rs) and a C compiler (tagteam bundles SQLite). The repository's
`rust-toolchain.toml` pins Rust 1.88.0, which rustup installs on first use.

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
one JSON object on stdout. The exceptions: `auto --json` prints one event per line,
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
| `run`, `map`, `unmap`, `shell-init` | Run Claude Code as an account in a separate session, and map directories to accounts |
| `history` | Usage history, burn rate, and when each window runs out |
| `statusline` | One line for Claude Code's status bar |
| `export`, `import` | Move accounts between machines (age-encrypted by default); `import` also reads claude-swap v1 exports |
| `config` | Read and edit `config.toml` (`list`, `get`, `set`, `unset`, `path`) |
| `doctor` | Check tagteam's state and its interop with Claude Code |
| `displaced` | List, or delete, credentials tagteam set aside rather than overwrite |
| `purge` | Delete tagteam's data (see [Uninstall](#uninstall-and-purge)) |
| `completions` | Print a completion script for bash, zsh or fish |

Exit codes: `0` success, `1` error, `2` usage error, and 128 plus the signal number when
interrupted (`130` for Ctrl-C). `tagteam auto --once` uses `0` switched, `1` error, `2` no
action and `3` blocked.

### Automatic switching

```sh
tagteam auto
```

`auto` runs in the foreground until you press Ctrl-C and prints a line per check. It needs at
least two switchable accounts. By default it switches away above 90% usage, checks every 60
seconds and waits 300 seconds between automatic switches. Change these with flags
(`--threshold`, `--interval`, `--cooldown`, `--strategy`) or with the settings
`autoswitch.threshold`, `autoswitch.interval_seconds`, `autoswitch.cooldown_seconds` and
`autoswitch.strategy`, for example `tagteam config set autoswitch.threshold 85`. tagteam has
no background service in this release, so keep `auto` running in a terminal multiplexer or
under a process supervisor of your choice.

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
and themes, and Claude Code's file history, paste cache, shell snapshots and session
environments. Any other entry, including the credential, stays private to the profile. To
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
| Store (account metadata, usage, history) | `$XDG_DATA_HOME/tagteam/tagteam.db` (default `~/.local/share/tagteam/`) |
| Vault on Linux | `$XDG_DATA_HOME/tagteam/vault/` |
| Session profiles | `$XDG_DATA_HOME/tagteam/sessions/` |
| Rescued and displaced credentials | `$XDG_DATA_HOME/tagteam/rescue/` and `displaced/` |
| Log | `$XDG_STATE_HOME/tagteam/tagteam.log` (default `~/.local/state/tagteam/`) |

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
- **The log** never contains an email, a token or any part of a credential.
- **Exports** are encrypted with age by default, using a passphrase or recipient keys.
  `--plaintext` is an explicit opt-out. An OAuth export hands a login over rather than copying
  it: whichever machine refreshes the token first invalidates the other copy. To use one
  account on two machines, log in on each.
- **Network.** The only hosts in tagteam's source are Claude Code's own: `platform.claude.com`
  (token refresh) and `api.anthropic.com` (profile and usage). `tagteam doctor --online` only
  checks that these hosts are reachable and sends no credential. `tagteam statusline` makes no
  network request.

See [SECURITY.md](SECURITY.md) for how to report a vulnerability.

## Uninstall and purge

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
