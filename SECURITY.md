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

Only the latest release receives security fixes.

## Verifying a release

Tagged releases carry GitHub artifact attestations for every release file. To check a file you
downloaded:

    gh attestation verify <file> --repo michaelrigart/tagteam

It confirms that the file was built by this repository's release workflow. A file that fails
the check should not be run; report it as described above.

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
- **Credentials and `security`'s command line.** tagteam passes a credential to
  `/usr/bin/security` on standard input (`security -i`), not on its command line. When a
  credential exceeds about 3.8 KB, `security` receives it on its command line instead, which
  macOS shows only to the same user and root but which exec-auditing tools record. Claude
  Code's own Keychain writes do the same. tagteam logs a WARN when it happens (the service,
  the account ID and the length, never the value). The vault and `rescue/` copies hold no
  machine-shared key (MCP OAuth tokens, plugin secrets), which keeps nearly every credential
  well below that size.
- **Passing a token.** `tagteam add-token <TOKEN>` puts the token on the command line, where
  other local users can see it and your shell history keeps it; tagteam prints a warning.
  Use `tagteam add-token -` (it reads standard input, with no echo on a terminal) or
  `tagteam add-token` with no argument (a prompt).
- **Session profiles** (`tagteam run`) hold their account's credential the way Claude Code
  keeps it: tagteam writes `sessions/<id>/.credentials.json` with mode 0600, and on macOS
  Claude Code moves it into its own Keychain item, `Claude Code-credentials-<hash>`, on its
  first credential write.
- **Files that hold secrets** are created with mode 0600 at creation time (exclusive create,
  write, rename) and are not changed afterwards. tagteam's directories are created with mode
  0700. `rescue/` (a refreshed token, written before its vault write and kept if that write fails)
  and `displaced/` (a credential tagteam would otherwise have overwritten) hold credential
  bytes in plain files with mode 0600 on both platforms. On macOS that means a refreshed token
  also sits in a 0600 file for about 100 ms per refresh, in a 0700 directory; it is encrypted
  at rest when FileVault is on, and a local snapshot taken inside that window keeps a token
  that the next refresh supersedes.
- **Claude Code's own state** is written only through a closed list: the account-scoped keys of
  the active credential, the `oauthAccount` value in `~/.claude.json`, and the few other writes
  listed in section 3 of the spec. Nothing else under `~/.claude/` is changed, and tagteam never
  edits Claude Code's `settings.json`.
- **`~/.claude.json` and API keys.** On Linux, and on macOS when the Keychain refuses the
  write, an API-key account's key is stored as `primaryApiKey` in `~/.claude.json`, the way
  Claude Code stores it itself. tagteam keeps that file's mode and never changes the mode of a
  Claude Code file. The file is often 0644, which lets other local users read the key. tagteam
  logs a WARN when it writes the key into a file that group or others can read; the fix is
  `chmod 600 ~/.claude.json` (the file in `CLAUDE_CONFIG_DIR` if that is set).
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
  case where every write fails, and never sends a generation it knows was superseded to the
  token endpoint. After an ambiguous failure (a timeout or reset with no response read) it
  deliberately retries the same generation, because the server may never have processed the
  request; if it had, the retry gets `invalid_grant` and the account is marked for a new
  login. A copy of a login exported to another machine stops working as soon as either machine
  refreshes it.
- **Root.** tagteam refuses to run as root, so it does not leave root-owned files in a user's
  directories. The one exception is a Linux container whose root filesystem is an overlay
  mount.
- **Purge.** `tagteam purge` deletes the data, the Keychain items and the log, and empties the
  store with `secure_delete` on and a `VACUUM`.
- **Dependencies.** CI runs `cargo deny check advisories` on every push and pull request and
  once a week, and fails on a RustSec advisory against a locked crate (a vulnerability or an
  unsound advisory anywhere in the graph; an unmaintained crate among tagteam's direct
  dependencies). The one advisory it ignores, and each reason, is recorded in `deny.toml`.

Anyone who can read your user account's files, or unlock your login Keychain, can read your
stored credentials, as they can read Claude Code's own. tagteam does not defend against that.

## Out of scope

- Vulnerabilities in Claude Code or in Anthropic's services. Report those to Anthropic.
- Issues that need an attacker who already runs code as your user or can read your Keychain.
