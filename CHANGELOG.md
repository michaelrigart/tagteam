# Changelog

All notable changes to tagteam are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions are calendar versions,
`YYYYMMDD.HHMM.0` in UTC: a version says when a release was made, not whether it is compatible
with the one before, so any release may contain breaking changes. Each section says what changed.

## [Unreleased]

### Changed

- `tagteam run` now also removes Claude Code's session-access, gateway and host-credential
  variables (`CLAUDE_CODE_SESSION_ACCESS_TOKEN`, `CLAUDE_CODE_GATEWAY_TOKEN`,
  `CLAUDE_CODE_HOST_CREDS_FILE` and `CLAUDE_CODE_HOST_AUTH_ENV_VAR`) from the session's
  environment, so none of them can give the session a login other than the account's own. It
  warns about each one that was set, and `tagteam doctor` reports them. Variables that only route
  traffic, such as `ANTHROPIC_BASE_URL`, are still passed through.

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

[Unreleased]: https://github.com/michaelrigart/tagteam/compare/v20261014.1500.0...HEAD
[20261014.1500.0]: https://github.com/michaelrigart/tagteam/compare/v20261013.2030.0...v20261014.1500.0
[20261013.2030.0]: https://github.com/michaelrigart/tagteam/releases/tag/v20261013.2030.0
