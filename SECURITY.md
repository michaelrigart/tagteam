# Security policy

## Reporting a vulnerability

Please report vulnerabilities privately, through GitHub's private vulnerability reporting:

1. Open the [Security tab](https://github.com/michaelrigart/tagteam/security) of the repository.
2. Choose **Report a vulnerability**, or go straight to
   [the report form](https://github.com/michaelrigart/tagteam/security/advisories/new).

Do not open a public issue or pull request for a security problem, and do not post credentials,
tokens or the contents of a tagteam data directory anywhere public. If you are unsure whether
something is a vulnerability, report it privately anyway.

tagteam is maintained by one person, so there is no fixed response time. Reports are read and
answered on a best-effort basis, and a confirmed issue is fixed in a new release and credited in
the advisory unless you ask otherwise.

## Supported versions

tagteam is pre-1.0. Only the latest release receives security fixes.

## What tagteam stores and protects

tagteam manages Claude Code logins, so it holds credentials. This is what it stores and how it
protects it. The full description is in the
[design spec](docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md) (sections 3, 5, 6
and 14).

- **Credentials** live in the vault. On macOS the vault is the login Keychain (generic
  passwords, service `tagteam`, an item per account plus one for its previous generation),
  reached only through `/usr/bin/security`. On Linux it is `vault/<id>.json` and
  `<id>.prev.json` with mode 0600 in a directory with mode 0700. The SQLite store
  (`tagteam.db`) holds metadata and usage readings, never a credential.
- **Session profiles** (`tagteam run`) hold their account's credential the way Claude Code
  keeps it: tagteam writes `sessions/<id>/.credentials.json` with mode 0600, and on macOS
  Claude Code moves it into its own Keychain item, `Claude Code-credentials-<hash>`, on its
  first credential write.
- **Files that hold secrets** are created with mode 0600 at creation time (exclusive create,
  write, rename) and are not changed afterwards. tagteam's directories are created with mode
  0700. `rescue/` (a refreshed token whose vault write failed) and `displaced/` (a credential
  tagteam would otherwise have overwritten) hold credential bytes in plain files with mode 0600
  on both platforms.
- **Claude Code's own state** is written only through a closed list: the account-scoped keys of
  the active credential, the `oauthAccount` value in `~/.claude.json`, and the few other writes
  listed in section 3 of the spec. Nothing else under `~/.claude/` is changed, and tagteam never
  edits Claude Code's `settings.json`.
- **Export files** are encrypted with age by default (a passphrase derived with scrypt, or
  `age1` and `ssh-ed25519` recipients). `--plaintext` is an explicit opt-out. The export is
  written with mode 0600.
- **Logs** at `$XDG_STATE_HOME/tagteam/tagteam.log` (mode 0600) never contain an email, a token
  or any part of a credential, a passphrase, or an export's contents.
- **Network.** The only hosts in tagteam's source are Claude Code's own OAuth hosts,
  `platform.claude.com` and `api.anthropic.com`, used over TLS for token refresh, the account
  profile and usage. There is no telemetry, and `tagteam statusline` makes no network request.
- **Refresh tokens are single-use.** tagteam persists every successor it receives (to the
  vault, to `rescue/`, or to `displaced/` when it belongs to another account), reports the
  case where every write fails, and never sends a superseded token to the token endpoint. A
  copy of a login exported to another machine stops working as soon as either machine
  refreshes it.
- **Root.** tagteam refuses to run as root, so it does not leave root-owned files in a user's
  directories. The one exception is a Linux container whose root filesystem is an overlay
  mount.
- **Purge.** `tagteam purge` deletes the data, the Keychain items and the log, and empties the
  store with `secure_delete` on and a `VACUUM`.

Anyone who can read your user account's files, or unlock your login Keychain, can read your
stored credentials, as they can read Claude Code's own. tagteam does not defend against that.

## Out of scope

- Vulnerabilities in Claude Code or in Anthropic's services. Report those to Anthropic.
- Issues that need an attacker who already runs code as your user or can read your Keychain.
- RUSTSEC-2023-0071 (RSA timing side channel) in the `rsa` crate, which `age` pulls in for its
  `ssh` feature. tagteam never decrypts with an RSA key: it refuses an RSA identity file and an
  `ssh-rsa` recipient, so the advisory does not apply.
