# tagteam — Directory mappings in `config.toml`

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/62
**Date:** 2026-11-11
**Scope:**
- Where `tagteam map` keeps the directory → account mappings that `tagteam run` and the shell
  wrapper use: `config.toml` instead of the database.
- `map`, `unmap`, `run`'s lookup, the settings read, `doctor`, the README and CHANGELOG.
- The shell wrapper and session profiles do not change.

**Builds on:** the core spec,
[`2026-09-26-tagteam-core-cli-design.md`](2026-09-26-tagteam-core-cli-design.md), cited as
"core §n". This spec amends core §6.1, §6.4, §10.3 and §12.7.

Line numbers are as of `688f8492` and will drift.

---

## 1. Purpose

### 1.1 Why

Michael asked on 2026-11-11 whether per-project accounts can be set in a dotfile. Today they
cannot. `tagteam map ACCOUNT [PATH]` stores each mapping in the database, in the `mappings`
table (core §6.1), keyed by a machine-local account ID. Only the `shell-init` line lives in a
dotfile. A user who keeps `~/.config` in a dotfile manager, as Michael does with chezmoi, cannot
carry their mappings to another machine or review them in one place.

He also chose the scope: the terminal only. Research on 2026-11-11 found:
- The VS Code extension runs its own bundled copy of Claude Code. It could be wrapped through
  `claudeCode.claudeProcessWrapper`, but its account panels have open bugs.
- The desktop app's Code tab signs in with the desktop app's own account and offers no
  per-folder choice.

Both are documented as not covered (§5).

### 1.2 What exists

- **Commands.** `map` canonicalises PATH and stores `(path, provider, account_id, added_at)`
  (`crates/tagteam/src/app/commands/mappings.rs:18-121`). `unmap` deletes by path
  (`:123-142`). `map` with no arguments lists the mappings.
- **Lookup.** `run` with no ACCOUNT looks up the nearest mapped ancestor of the canonical cwd,
  matching whole path components (`crates/tagteam-engine/src/run.rs:167-181`,
  `store/mod.rs:1864-1892`). It handles three cases:
  - a mapping to a removed account: a warning, then plain `claude`;
  - no mapping: plain `claude`;
  - `--require-session`: refuses both (core §12.1).
- **Settings.** `config.toml` already holds one table that is not a registry key,
  `provider.<id>.prices` (`settings.rs:904-908`). It is read forgivingly, its bad entries are
  warned about, and `config list` leaves it out.

## 2. Goals

- **G1.** Mappings can be written by hand in `config.toml`, and read the same on every machine
  that has the accounts.
- **G2.** `tagteam map` and `unmap` edit the file, so the file is the one place to look.
- **G3.** Existing database mappings keep working until each one is mapped again. Nothing writes
  the file unasked.
- **G4.** A mapping that does not apply on this machine (a missing directory, an unknown
  account) never stops `claude` from starting. It falls back to plain `claude` as today, with a
  warning when the mapping matched.

### Non-goals

- IDE extensions and the desktop app (§1.1).
- A per-repository file (such as `.tagteam.toml`) or a top-level `[mappings]` table.
  Mappings stay per provider, as the database's are.
- Removing the database table, or an automatic migration.
- Changing the shell wrapper, `run`'s session behaviour, or `--require-session`.

### Anti-goals

- Writing the user's `config.toml` without a command that says so.
- A mapping entry that names an account by its machine-local ID or position. That would defeat
  G1.

## 3. Design

### 3.1 The file

```toml
[provider.claude-code.mappings]
"~/Code/work" = "work"                       # an alias
"/Volumes/Data/client-x" = "alex@client.com" # an email
```

- **Key: a directory.**
  - `~` alone or `~/…` expands to the user's home directory. Otherwise the key must be
    absolute.
  - A relative or empty key is ignored with a warning naming it, as an invalid setting is
    (core §6.4).
  - Whether the directory exists is judged at lookup (§3.2), not at read. A dotfile may serve
    several machines.
- **Value: an account, by alias or email.** It is resolved the way an ACCOUNT argument is
  resolved (`crates/tagteam-engine/src/refs.rs:11-35`: a position, then an alias, then an exact
  email), restricted to the table's provider. A value that names no account, or more than one,
  does not apply (§3.2).
  - **A value that reads as a position is never resolved.** The entry is kept, marked with the
    problem `position`, and it takes part in the lookup like any entry (§3.2). If it wins for a
    directory, it warns and falls back like every entry that does not apply, so it cannot hand
    its directory to a database mapping either. Positions differ between machines and change
    with `move`, so resolving one would launch another account without a word. An alias cannot
    read as a position (`refs.rs` tries a position first). An account ID does not resolve
    either. *(Changed from the spec review.)*
- **A mapping follows the name, not the account.** If an alias is renamed, unset or given to
  another account, or the account is removed, the entry follows the name. `alias` and `remove`
  say so when it happens (§3.6). This is the price of a file that reads the same on every
  machine. *(My call, from the builder check.)*
- **Reading.** The read is forgiving and lives where `prices` is read. `Settings` holds the
  entries for the provider it was loaded for, as written. `config list`, `get` and `set` do not
  show or change them.
- **Unknown keys.** The unknown-key check (`settings.rs:1343-1416`) treats every key under
  `provider.<id>.mappings` as known.

### 3.2 Lookup (`run`, core §12.7)

- The candidates are the database's mappings for the provider and the file's entries whose
  directory exists. Each directory is expanded and canonicalised as `map` canonicalises.
- The nearest whole-component ancestor of the canonical cwd wins, across both sources. When both
  sources map the same directory, the file wins.
- **Equivalent file entries.** Two or more file entries can canonicalise to one directory, for
  example `~/Code/work` and `/Users/michael/Code/work`, or a path and a symlink to it.
  - With the same value, they are one mapping.
  - With different values, the file's mapping of that directory does not apply. The lookup warns
    naming every entry and runs plain `claude`, or `--require-session` refuses. A database
    mapping of the same directory does not step in, since the file wins for that directory.
  - `doctor` reports the conflict.
- **The winner resolves to an account:** a session as today.
- **The winner is a file entry that does not apply here:** a warning naming the entry and the
  file, then plain `claude`. That covers a value that names no account, several, an API-key
  account or a position. A missing directory never wins, since such entries are not
  candidates. This is today's "mapped to a removed account"
  path, worded for the file. `--require-session` refuses, as it does there. An API-key account
  takes this path rather than `run`'s `ApiKeyAccount` error (`run.rs:124-128`). A shared file
  can name an account that is OAuth on one machine and an API key on another, and G4 says
  `claude` still starts. A database mapping keeps today's error. *(My call, from the builder
  check.)*
- **Entries whose directory does not exist** on this machine are skipped silently at lookup.
  `doctor` reports them (§3.5).
- `run` sits on the wrapper's path for every `claude` start, so the lookup does no network or
  Keychain work. Canonicalising each entry is a filesystem call per entry, which is acceptable
  for tens of entries.

### 3.3 `map ACCOUNT [PATH]`

- **Writes** `provider.<p>.mappings` in `config.toml`, under the settings lock, with the atomic
  writer and `toml_edit`, as `config set` writes (core §6.4).
- **The key** is PATH canonicalised, as today, written as `~/…` when it is under the home
  directory and absolute otherwise.
- **The value** is the account's alias, else its email, and it must resolve back to exactly
  this account.
  - It is refused, saying to give the account an alias first, when there is neither, or when the
    email names more than one account. Claude Code's identity is the email and the org
    (`crates/tagteam-cc/src/provider.rs:320-321`), so one email can be several accounts.
  - An API-key account is refused, since `run` never starts a session for one.
  - *(My call, from the builder check.)*
- **Entries already in the file for the same directory** (compared after expansion and
  canonicalisation, not as text) are all replaced. The first one is rewritten in place, with its
  comments, and every other equivalent entry is removed. The file's other content stays.
- **A database mapping for the same directory and provider** is deleted in the same command, so
  the file is the only source for that directory.
- **Output** names the file: "Mapped ~/Code/work to work (position 2) in
  ~/.config/tagteam/config.toml." `--json` adds `"source": "config"` to the mapping it returns.
- **Inside a run shell**, `map` works as `config` does (core §6.4, §12.8).

### 3.4 `unmap [PATH] [--provider P]` and the listing

- **`unmap`** removes the file entries for PATH (compared as in §3.3; for a directory that is
  gone, by its expanded absolute path) and the database rows, for `--provider`'s provider or
  all. `removed` counts both sources. An entry that leaves its table empty takes the table with
  it, as `config unset` does.
- **`map` with no arguments** lists both sources, sorted by path, one line each. A database row
  says "(database; `tagteam map` moves it to the file)". A file entry that resolves to no account
  says so.
- **`--json`.** Only additions under `schemaVersion: 1`, and no field of today's changes type.
  - **`mappings`** keeps today's objects and fields exactly (`render.rs:531-544`), for the
    database rows only.
  - **`configMappings`** is a new array, one object per file entry:
    - `path`: the expanded, canonical directory, or the expanded absolute path when it does not
      exist here;
    - `written`: the key as written, such as `~/Code/work`;
    - `value`: the value as written;
    - `provider`;
    - `account`: `{number, id, email, alias?}` when the value resolves to exactly one account,
      else absent;
    - `problem`, when it does not apply: `missing-directory`, `no-account`, `ambiguous`,
      `api-key`, `position` or `conflict`.
  - **`map`'s own result** keeps today's `mapping` object, all fields present. `addedAt` is the
    command's time, which is when the entry was added. It gains `"source": "config"`.
  - A script that read `mappings` sees an entry leave it when `map` moves the entry to the file.
    The README and the CHANGELOG say so.
  - *(Changed from the spec review.)*

### 3.6 `alias` and `remove`

Neither edits the file (anti-goal). Each warns when it changes what a file entry resolves to,
naming each entry and the file:
- **`remove`** warns about entries whose value named the removed account. Those directories
  will now run plain `claude` with a warning. Its database rows still go with it (ON DELETE
  CASCADE, core §10.3).
- **`alias`** (set, rename or unset) warns about entries that resolved to some account before
  and resolve to another account, or to none, after. That includes an alias moved to another
  account, which would otherwise launch that directory as the other account without a word.

### 3.5 `doctor`

- **Info:** a file entry whose directory does not exist here.
- **Warning:** a file entry whose value names no account, several, an API-key account or a
  position, and equivalent entries with different values.
- **Info:** the number of database mappings left, with "`tagteam map` moves each to
  `config.toml`".

## 4. Tests

- **Read:**
  - `~` and `~/…` expand;
  - a relative or empty key gives a warning and is ignored;
  - a value is kept as written;
  - the unknown-key check passes mapping keys, and `config list` does not show them.
- **Lookup:**
  - nearest across sources, with the deeper one winning whichever source holds it;
  - the same directory in both sources goes to the file;
  - a symlinked cwd finds a file entry;
  - a missing directory is skipped;
  - an unknown or ambiguous value gives a warning and plain `claude`, and `--require-session`
    refuses it;
  - alias and email both resolve.
- **`map`:**
  - writes the `~/` form and the alias, else the email;
  - refuses an account with neither, an email that names two accounts, and an API-key account;
  - replaces in place, keeping comments;
  - deletes the database row;
  - works with no store yet only where it does today.
- **`unmap`:** removes from both sources and counts both, and drops an empty table.
- **Listing:** human and `--json`, both sources. `mappings` is unchanged, and `listed_shapes`
  passes as it is. `configMappings` covers every `problem` value.
- **Position values:** a file entry valued `"2"` for a directory the database also maps wins,
  warns and runs plain `claude`. It never resolves, and it lists with `problem: position`.
- **Equivalent entries:** with the same value they apply. With different values the lookup warns
  and runs plain `claude`. `map` leaves exactly one entry for the directory.
- **Lookup, API key:** a file entry naming an API-key account gives a warning and plain
  `claude`.
- **`alias` and `remove`:** each warning case in §3.6. `removing_an_account_removes_its_mappings`
  stays, for database rows.
- **`doctor`:** all three lines.
- **End to end:** the wrapper in a mapped directory starts a session from a file entry.

## 5. Records

- **Core spec:**
  - §12.7: mappings live in `config.toml`, the database is read as a legacy source, and the
    lookup and precedence rules are §3.2's;
  - §6.4: the `provider.<id>.mappings` table, next to the note on `prices`;
  - §6.1's `mappings` comment: legacy, read only by `run` and `map`'s listing, deleted per
    directory by `map` and `unmap`;
  - §10.3 (`remove`, `alias`): the cascade covers database rows, and the warnings in §3.6.
  Each passage gets "*(Amended by the config-mappings spec.)*".
- **README, "Running accounts side by side":**
  - the `config.toml` form, with a dotfile-manager note: `map` writes the deployed file, so edit
    the source instead, or re-add after `map`;
  - that the IDE extensions and the desktop app do not go through the wrapper. The desktop
    app's Code tab uses its own sign-in.
- **CHANGELOG.**

## 6. Left to the builder

- How `Settings` carries the entries, and whether the lookup lives in the engine's `run` or a
  helper shared with `map` and `unmap`.
- How a directory comparison is shared between `map`, `unmap` and the lookup.
- Exact message wording within §3's meaning, and the test fixtures.
