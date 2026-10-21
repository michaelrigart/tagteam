# Changelog

All notable changes to tagteam are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions are calendar versions,
`YYYYMMDD.HHMM.0` in UTC: a version says when a release was made, not whether it is compatible
with the one before, so any release may contain breaking changes. Each section says what changed.

## [Unreleased]

### Added

- Release files carry GitHub artifact attestations; check one with `gh attestation verify`.
- CI checks RustSec advisories with `cargo-deny`.
- A WARN when an API key is written into a `~/.claude.json` that other users can read.
- `tagteam add-token <TOKEN>` prints a one-line warning on stderr that the token was visible on
  the command line; `tagteam add-token -` and the prompt avoid that.

### Changed

- `--full` exports no longer carry the machine-shared keys (Claude Code's MCP OAuth tokens and
  plugin secrets); they stay on the machine, as default exports already did.
- `tagteam add-token -` on a terminal no longer echoes the token.
- Human output replaces control characters with U+FFFD in a session's project directory and in
  unmanaged login emails; `--json` and CSV keep both raw. The token endpoint's error text is
  sanitized wherever it appears, `--json` warnings included.
- Keychain writes send the credential as text on `security`'s standard input, which roughly
  doubles what fits before the command-line fallback; that fallback now logs a WARN.
- A refreshed token is written to `rescue/` before the vault write, so a kill during the vault
  write no longer loses it.
- The daemon and `tagteam run` wake far less often when idle. A 12-month token report is about
  twice as fast and uses less than half the memory.
- `tagteam tui` no longer wakes up 20 times a second while it waits for a key.
- `tagteam tui`'s header says auto's state and threshold, and a line under it says auto's last
  action in plain words.
- `tagteam tui` marks the selected row with a bar instead of reverse video, and draws in truecolor
  where the terminal supports it and reports its background.
- `tagteam tui`'s footer always shows `? keys` and `q quit`. Help lists every key and what each
  mark means.
- `tagteam tui` reports a successful action in a toast. Doctor, and output too long for a toast,
  open a panel. `r` says what it updated.
- `tagteam tui` asks before `R` starts a session and before it hands the terminal over to unlock
  the keychain. It ignores action keys for 300 ms after a dialog closes.
- `tagteam tui` shows the settings warnings in a banner, and keeps the last token report on screen
  when a new one cannot be read.
- `tagteam tui`'s Accounts screen no longer binds `g` and `G`, and `q` closes an open overlay
  before it quits.
- `tagteam config set default_provider` refusing an invalid id, and the warning for an invalid
  `default_provider` in config.toml, no longer give `claude-code` as an example: they read "must
  be a provider id of lowercase letters, digits and dashes".
- `tagteam --help` and each command's help describe what works for every provider without
  naming Claude Code. The help for the statusline and the shell wrapper, which only Claude
  Code has, still names it.
- `tagteam completions` exits with an error when it cannot write the script, rather than exiting 0
  with a partial one; a reader that closes the pipe early is still not an error.
- The daemon no longer wakes every 60 s because of an account its usage collection skips (one a
  `tagteam run` session owns, or a managed API key); it waits for the next account a round will
  collect.
- When `tagteam run` cannot refresh an account's token because a pending rescue cannot be read,
  it now names the damaged files; it said only that a pending rescue could not be adopted.

### Security

- The daemon client refuses a socket served by another user.
- rustls is updated to 0.23.46 (RUSTSEC-2026-0346 to RUSTSEC-2026-0349).
- rustls-webpki is updated to 0.103.16, and webpki-roots and webpki-root-certs to 1.0.10
  (RUSTSEC-2026-0381 to RUSTSEC-2026-0383).

## [20261016.2230.0] - 2026-10-16

### Added

- `tagteam tokens`: token use and its API-equivalent cost from Claude Code's transcripts, by
  day, month, session or 5-hour block, per account and per model, as a table, `--json` or
  `--csv`. Each request is credited to the account that made it where tagteam can tell (a
  `tagteam run` session, or the default login at the time), and shown as unattributed where it
  cannot. tagteam keeps the counts, never any content, in an index in its store, so the history
  outlives Claude Code's 30-day transcript cleanup; `tokens.retention_days` (default 365) bounds
  it. Prices ship with tagteam, and `[provider.claude-code.prices."<model>"]` tables in
  `config.toml` override them.
- `tagteam run` and `tagteam statusline` now record which session runs under which account, for
  the token report. Neither changes what it prints or its exit code, and the status line stays
  within its time budget.
- `tagteam daemon`: a background process that keeps the usage of the live account and the
  switchable accounts fresh and switches accounts automatically, as `tagteam auto` does, for
  each provider with two or more switchable accounts. Every command still works without it.
- The daemon keeps the `tagteam tokens` index current. It records running Claude Code sessions
  every 5 seconds and indexes new transcript lines every minute, and its socket announces each
  change with a `token-index` event.
- `tagteam daemon install`, `uninstall` and `status`. `install` sets up a LaunchAgent on macOS
  or a systemd user unit on Linux, which starts the daemon at login and restarts it after a
  failure or an upgrade. `install --print` prints the unit instead.
- Notifications from the daemon when it switches accounts, when an account needs a new login,
  and when the live account reaches the switch threshold and the daemon does not switch away
  from it: through `osascript` on macOS and `notify-send` on Linux.
- Settings `daemon.autoswitch` (pause the daemon's switching, per provider), `notify.enabled`
  and `notify.events`.
- A local socket, `daemon.sock` (mode 0600, your user only), that streams the daemon's events
  and answers status requests, for the TUI and for scripts.
- `tagteam doctor` reports the daemon, its service unit, its socket and its notifications.
- The internal `tagteam-daemon` crate on crates.io.
- `tagteam tui`, a terminal UI: the accounts table as `tagteam list` shows it, with gauges and
  a detail pane for the selected account, and usage history with a sparkline per window. Its
  keys run tagteam's own commands (switch, enable and disable, alias, move, remove, add and
  doctor), and `R` runs a session as `tagteam run` does. It refreshes every 30 seconds, and
  collects usage only when `list` would. On a terminal, a bare `tagteam` now opens it; in a
  pipe, or with `--json`, it still runs `list`.
- The TUI's Tokens screen (`2`, or `tagteam tui tokens`) shows the token report by day,
  month, session or 5-hour block, grouped and filtered by account and model, with drill-down
  from a period to its sessions and from a session to its models, and the active block's remaining
  time, burn rate and projection. It brings the token index up to date in the background, and reads the
  report again as the index, the accounts or the clock move.
- With the daemon running, the TUI follows it live. The header shows what the daemon does for
  the selected account's provider (`● live · auto on`, `auto paused`, …), the line above the
  footer shows its latest event, and the TUI reads its views again as the daemon's events come
  in. It leaves collecting usage, and keeping the token index current, to the daemon. `P`
  pauses or resumes the daemon's auto-switching for that provider, as
  `tagteam config set provider.<id>.daemon.autoswitch` does. Without a daemon, or once it
  stops, the TUI refreshes on its own timer, and it connects again when a daemon is back.

### Changed

- `tagteam purge` waits for a running token index refresh before it deletes a provider's data,
  and deletes the index with it. `tagteam remove` keeps a removed account's token usage.
- `tagteam auto` runs on the daemon's scheduler. Its behaviour, output and exit codes are
  unchanged. When the daemon already switches a provider, `auto` says so and names the command
  that pauses it.
- An auto-switch engine that stops now empties its lock file's record, so `tagteam doctor`
  never names a process that no longer drives the provider.

## [20261014.2045.0] - 2026-10-14

### Changed

- `tagteam run` now also removes Claude Code's session-access, gateway and host-credential
  variables (`CLAUDE_CODE_SESSION_ACCESS_TOKEN`, `CLAUDE_CODE_GATEWAY_TOKEN`,
  `CLAUDE_CODE_HOST_CREDS_FILE` and `CLAUDE_CODE_HOST_AUTH_ENV_VAR`) from the session's
  environment, so none of them can give the session a login other than the account's own. It
  warns about each one that was set, and `tagteam doctor` reports them. Variables that only route
  traffic, such as `ANTHROPIC_BASE_URL`, are still passed through.
- `tagteam run` sessions now share Claude Code's task lists (`tasks`) with your default home,
  like its file history and session environments, so a session resumed in another account keeps
  its tasks. Mods in development (`dev-mods`) stay per profile, so Claude Code's sandbox keeps
  protecting them. `tagteam doctor` and the launch no longer report either entry as
  unknown.

## [20261014.1500.0] - 2026-10-14

### Changed

- Checked against Claude Code 2.1.299, now the newest version tagteam is known to work with.
  `tagteam doctor` no longer warns about Claude Code versions up to 2.1.299.

## [20261013.2030.0] - 2026-10-13

First release. tagteam is a multi-account switcher for Claude Code on macOS and Linux.

### Added

- Account storage and switching: `add`, `add-token` (API keys and setup tokens), `remove`,
  `disable`, `enable`, `alias`, `move`, `list`, `status` and `switch`. A switch changes only the
  account identity; Claude Code's memory, history, settings and other local state are left
  alone. An interrupted switch is recovered the next time a command runs.
- A vault for credentials: the login Keychain on macOS (through `/usr/bin/security`) and mode
  0600 files on Linux. Every refresh-token successor is persisted: to the vault, to a rescue
  file, or under `displaced/` when it belongs to another account. If every write fails,
  tagteam reports it. A credential that was not tagteam's to keep is also set aside under
  `displaced/` (`tagteam displaced`).
- Token refresh and usage: OAuth token refresh with quarantine of accounts that need a new
  login, and usage collection with a request budget and failure backoff. `list` shows the 5-hour
  and 7-day windows, spend, per-model limits and reset countdowns for every account.
- Switch strategies: `switch --strategy best` and `--strategy next-available`, with `--model`
  to choose which model limits count.
- `auto`: automatic switching before a rate limit, with a threshold, interval, cooldown and
  hysteresis, the `best` and `consume-first` strategies, `--once` and `--dry-run`, and JSON
  events.
- `run`: Claude Code as another account in its own session, beside the default login, with
  history, projects, memory and settings shared with the default home. `map`, `unmap` and
  `shell-init` choose the account by directory and wrap `claude` in bash, zsh and fish.
- `history`: stored usage samples with burn rate, projected exhaustion and CSV output.
- `statusline`: a fast status line for Claude Code, with `--print-config` for the
  `settings.json` snippet and a configurable `statusline.format`.
- `config`: `list`, `get`, `set`, `unset` and `path` for `config.toml`, with comment-preserving
  edits and per-provider overrides.
- `export` and `import`: age-encrypted by default (passphrase, `age1` or `ssh-ed25519`
  recipients), a plaintext opt-out, and import of claude-swap version 1 exports.
- `doctor`: read-only checks of tagteam's state and its interop with Claude Code, including a
  warning for a Claude Code version newer than the one tagteam was checked against
  (2.1.292). `doctor --online` also checks that the endpoints are reachable.
- `purge`: deletes tagteam's data and Keychain items, and never the live login.
- `completions` for bash, zsh and fish, and `--json` output for scripting (one object per
  command, one event per line for `auto`).
- A file log at `$XDG_STATE_HOME/tagteam/tagteam.log` that never records an email or a
  credential, with `--debug` and `TAGTEAM_LOG`.
- `cargo xtask compat`, a compatibility check against the real `claude`, and a weekly CI job
  that watches the latest Claude Code release for drift.
- Release builds for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-musl`
  and `aarch64-unknown-linux-musl`, a Homebrew formula in `michaelrigart/homebrew-tap`, and the
  `tagteam` crate on crates.io (`cargo install tagteam --locked`) with the internal
  `tagteam-*` crates it depends on.

[Unreleased]: https://github.com/michaelrigart/tagteam/compare/v20261016.2230.0...HEAD
[20261016.2230.0]: https://github.com/michaelrigart/tagteam/compare/v20261014.2045.0...v20261016.2230.0
[20261014.2045.0]: https://github.com/michaelrigart/tagteam/compare/v20261014.1500.0...v20261014.2045.0
[20261014.1500.0]: https://github.com/michaelrigart/tagteam/compare/v20261013.2030.0...v20261014.1500.0
[20261013.2030.0]: https://github.com/michaelrigart/tagteam/releases/tag/v20261013.2030.0
