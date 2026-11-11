# Directory mappings in `config.toml`: implementation plan

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/61

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** `[provider.<id>.mappings]` in `config.toml` maps directories to accounts by alias or
email. `map` and `unmap` edit it, `run` looks it up alongside the database's legacy rows, and
`doctor`, `alias` and `remove` report on it.

**Architecture:** `settings.rs` reads the table forgivingly into `Settings::mappings`, with
`~` expanded and nothing canonicalised. A new engine module, `mappings.rs`, owns three things:
- where an entry's directory is on this machine (`mapped_dir`);
- what its value names (`judge_value`);
- the judged list (`judge`, `Engine::config_mappings`).

`run.rs`, `config.rs`'s writers, `doctor` and the CLI all use that module, so one directory
comparison and one resolution serve every caller. `config.rs`'s lock and two-pass write become
one helper, shared by `config set`/`unset` and the two mapping writers.

**Tech Stack:** Rust 2024, MSRV 1.89, toolchain 1.99.1 through mise; `toml_edit`, `rusqlite`.
Tests use `assert_cmd` against the real binary (`--features tagteam/test-support`) and the
engine's `Fx` fixture.

**Spec:** `docs/superpowers/specs/2026-11-11-tagteam-config-mappings-design.md`, cited "S§n".
The core spec is cited "core §n". Line numbers are as of `18e2260f` and will drift.

## Global Constraints

- No new dependency. `Cargo.lock` does not change.
- `--json` changes are additions under `schemaVersion: 1`:
  - `map`'s listing keeps `mappings` exactly as today, for database rows only, and adds
    `configMappings`;
  - `map`'s result keeps its `mapping` object, every field, and adds `"source": "config"`;
  - `unmap`'s result keeps its shape.
- Nothing writes `config.toml` except `map`, `unmap`, `config set` and `config unset` (S anti-goal).
  `alias` and `remove` only warn.
- `run`'s lookup does no network or Keychain work, takes no lock, and creates no store or file. It
  canonicalises each entry, but resolves only the entry that wins.
- A value that reads as a position (`parse_account_ref` → `Position`) is never resolved (S§3.1).
- The read is forgiving. Every warning starts with the settings file's full path, names the dotted
  key, and ends `(ignored)`, as `Reader`'s warnings do.
- The log never records a mapping's path or value (core §14.2). A write logs
  `key = "provider.<id>.mappings"` and an action, nothing more.
- `problem` strings, exactly: `missing-directory`, `no-account`, `ambiguous`, `api-key`,
  `position`, `conflict`.
- Doctor check ids, exactly:
  - `mappings.directory` (info);
  - `mappings.value` (warn);
  - `mappings.conflict` (warn);
  - `mappings.database` (info).
- Commit messages are one plain imperative sentence, with no prefix and no attribution.
- Every commit passes three checks:
  - `cargo fmt --all --check`;
  - the three clippy runs in CONTRIBUTING.md, with `-D warnings`;
  - the focused tests of what it changed.

  Task 9 runs the full suite.
- CLI tests that spawn ptys or bind sockets fail inside the sandbox (`openpty: Operation not
  permitted`, `EPERM`). Run them unsandboxed. Never run `mise trust`.
- Doc comments cite the spec as their neighbours cite theirs ("config-mappings spec §3.2").

## Review Focus

1. **A home that is not canonical.** macOS temp dirs (`/var` → `/private/var`), or a `$HOME`
   reached through a symlink. `map` must still write `~/…` for a directory under the home, and
   that key must expand back to the same canonical directory. Task 4,
   `map_writes_a_home_relative_key_when_the_home_is_a_symlink`.
2. **Hand-written spellings of one directory.** A trailing `/`, a `.` component, or a symlink.
   Each matches the directory it names, in the read, the lookup and `map`'s replacement. Task 1,
   `mapping_keys_expand_the_home_and_keep_values_as_written` (trailing `/`). Task 3,
   `a_trailing_slash_or_symlinked_key_maps_its_directory`.
3. **A key that names a file, not a directory.** It is never a candidate, and it lists as
   `missing-directory`. Task 2, `a_key_naming_a_file_is_a_missing_directory`.
4. **A hand-written inline or dotted table** (`mappings = { "~/x" = "a" }` under
   `[provider.claude-code]`). The read takes it. `map` and `unmap` edit it in its own style.
   Task 4, `map_and_unmap_keep_an_inline_mappings_table_inline`.
5. **A `config.toml` that is not valid TOML when `claude` starts.** The read falls back to the
   defaults with its warning, and a database mapping still applies. `map` refuses to write.
   Task 3, `a_corrupt_settings_file_leaves_database_mappings_working`. Task 4,
   `a_corrupt_file_is_never_written`.

## Rulings made while planning (inside S)

- **Which providers `Settings` carries.** It holds only the provider it was loaded for, as S§3.1
  says. Warnings about another provider's table show only when a command acts on that provider,
  as for every per-provider key.
- **Every mapping read takes the file as it stands,** through `Settings::load(env, p)`, as doctor's
  `auto` check already reads. That covers `run`'s lookup, the listing, `doctor`, and the
  `alias`/`remove` warnings. None of them reads the engine's settings snapshot.
  - `run` plans a second time after a `TargetChanged` (`app/commands/launch.rs`). That second
    plan must see an `unmap` made meanwhile, as it already sees the store's.
  - The second parse costs one small file read on `claude`'s start.
- **`map` replacing an entry for the same directory.** The first such entry keeps its key as
  written and its comments; only its value changes. Every other equivalent entry is removed. A new
  entry gets the `~/…` or absolute key. S§3.3's "rewritten in place" is read as the entry's
  place and comments.
- **A key whose path exists but is not a directory** is `missing-directory`.
- **A value that is not a string** is an invalid setting: warned about and ignored (core §6.4),
  not kept with a problem.
- **Problem precedence** for one entry: `missing-directory`, then `conflict`, then `position`,
  then what the value names (`no-account`, `ambiguous`, `api-key`).
- **"The same value"** for equivalent entries means equal as written.
- **The `alias` and `remove` warnings are best-effort.** A failed read of the file's entries skips
  the warnings and never blocks the command. They belong to the CLI's `alias` and `remove`
  commands (S§3.6), not the TUI.
- **`doctor` with an unusable store** judges directories only, and skips values. `store.*`
  reports the store.
- **`map`'s refusal order:**
  1. no such account (as today);
  2. PATH (as today);
  3. an API-key account;
  4. neither alias nor email, or an email that names more than one account.
- **`map`'s message** names the settings file by its full path, as every settings warning does.
- **Task order.** Tasks run one at a time. Every task builds `tagteam-engine`, so no two have
  disjoint test builds.

## Files

| File | Responsibility |
|---|---|
| `crates/tagteam-engine/src/settings.rs` | `MappingEntry`, `Settings::mappings`, `expand_mapping_key`, the read, the unknown-key check |
| `crates/tagteam-engine/src/mappings.rs` (new) | `MappedDir`, `mapped_dir`, `MappingProblem`, `ConfigMapping`, `judge`, `Engine::config_mappings`, `Engine::judge_value` |
| `crates/tagteam-engine/src/refs.rs` | the text branch of account resolution, shared with `mappings.rs` |
| `crates/tagteam-engine/src/run.rs` | the lookup across both sources (S§3.2) |
| `crates/tagteam-engine/src/config.rs` | `write_mapping`, `remove_config_mappings`, `mapping_key`, the shared locked edit |
| `crates/tagteam-engine/src/doctor/mappings.rs` (new) | S§3.5's checks |
| `crates/tagteam/src/app/commands/mappings.rs` | `map`, `unmap`, the listing |
| `crates/tagteam/src/render.rs` | the listing's text and JSON |
| `crates/tagteam/src/app/commands/accounts.rs` | `alias` and `remove` warnings |
| `docs/…/2026-09-26-tagteam-core-cli-design.md`, `README.md`, `CHANGELOG.md` | S§5's records |

## Order

| Task | Depends on | Tier |
|---|---|---|
| 1 The read | — | sp-mechanical |
| 2 Judging entries | 1 | sp-standard |
| 3 `run`'s lookup | 2 | sp-standard |
| 4 Writing the file | 2 | sp-standard |
| 5 `doctor` | 2 | sp-mechanical |
| 6 `map`, `unmap` and the listing | 3, 4 | sp-standard |
| 7 `alias` and `remove` warnings | 6 | sp-mechanical |
| 8 End to end and the records | 6, 7 | sp-mechanical |
| 9 Verification | 1–8 | controller |

---

### Task 1: The read

**Files:**
- Modify: `crates/tagteam-engine/src/settings.rs`. Touch `Settings` and its `Default`,
  `read_file`, and `Reader` (a new `read_mappings`, beside `read_prices`). Add one arm to
  `place` (`:1343`).
- Test: `crates/tagteam-engine/tests/settings.rs`. Use its `load_as` helper, and `inspect` for
  the unknown-key check.
- Test: `crates/tagteam/tests/config_cli.rs` (`config list`).

**Interfaces:**
- Produces:
  - `pub struct MappingEntry { pub written: String, pub dir: PathBuf, pub value: String }`,
    deriving `Debug, Clone, PartialEq, Eq`. `dir` is the key expanded and rebuilt from its
    components (no trailing `/`), not canonicalised.
  - `pub mappings: Vec<MappingEntry>` on `Settings`: the loaded provider's
    `[provider.<id>.mappings]`, in file order. Empty by default.
  - `pub fn expand_mapping_key(home: &Path, key: &str) -> Option<PathBuf>`. `~` gives `home`, and
    `~/rest` gives `home.join(rest)`. Any other absolute key is kept as given. All of these are
    rebuilt from their components. An empty key, a relative one, or `~user/…` gives `None`.

- [ ] **Step 1: Write the failing tests** (`tests/settings.rs`)
  - `mapping_keys_expand_the_home_and_keep_values_as_written`. Under
    `[provider.claude-code.mappings]`: `"~" = "home"`, `"~/Code/work" = "Work"`,
    `"/abs/x/" = "a@x.co"`.
    - `mappings` equals those three `MappingEntry`s in order, with `dir`s `env.home`,
      `env.home.join("Code/work")` and `/abs/x`. Each `value` is as written (`"Work"` keeps its
      case).
    - There are no warnings.
  - `a_relative_or_empty_mapping_key_warns_and_is_ignored`. Keys `""`, `"Code/x"`, `"~other/x"`,
    each = `"a"`, and `"/ok" = 2`. `mappings` is empty, with four warnings. Each warning contains
    its key's text and ends `(ignored)`.
  - `mappings_are_read_for_the_loaded_provider_only`. Entries under
    `provider.fake-agent.mappings` are absent from `load_as(text, "claude-code")` and present
    from `load_as(text, "fake-agent")`.
  - `a_mappings_value_that_is_not_a_table_warns`: `[provider.claude-code]` holding
    `mappings = 3`. One warning, `` `provider.claude-code.mappings` must be a table (ignored)``.
  - `mapping_keys_are_known_and_a_top_level_mappings_table_is_not`. With mapping entries,
    `inspect(...).unknown` is empty. Adding `[mappings]` holding `"/x" = "a"` gives
    `unknown == ["mappings./x"]`, its dotted name as `every_key` builds it.

- [ ] **Step 2: Write the failing CLI test** (`tests/config_cli.rs`)
  - `config_list_leaves_mappings_out`: a file with a mapping entry. `config list` and
    `config list --json` succeed, their stdout does not contain `~/Code/work`, and stderr is empty.

- [ ] **Step 3: Run them to verify they fail**
  - `cargo test -p tagteam-engine --test settings mapping` fails to compile: there is no
    `mappings` field.
  - `cargo test -p tagteam --features test-support --test config_cli config_list_leaves` fails on
    an unknown-key warning in stderr.

- [ ] **Step 4: Implement**
  - `read_file` sets `read.settings.mappings` from
    `reader.read_mappings(provider, &env.home)`.
  - `read_mappings` reads `["provider", id, "mappings"]` through `Reader::table`, so a
    non-table warns as other tables do. It keeps each string value whose key expands, and warns
    otherwise:
    - a bad key: `` `provider.<id>.mappings."<key>"` must be an absolute directory or start with
      `~/` (ignored)``;
    - a non-string value: `` `provider.<id>.mappings."<key>"` must name an account by alias or
      email, as a string (ignored)``.
  - `place`: `["provider", _, "mappings"]` is `Place::Table`, and
    `["provider", _, "mappings", _, ..]` is `Place::Key`. Both arms go before
    `["provider", _, rest @ ..]`.
  - The field's doc comment says, as `prices`'s does, that it is not a registry key, so `config`
    neither lists nor writes it.

- [ ] **Step 5: Run the tests to verify they pass**
  - The two commands of Step 3, then `cargo test -p tagteam-engine --test settings`, all
    passing.

- [ ] **Step 6: Commit**: `Read directory mappings from config.toml`.

---

### Task 2: Judging entries

**Files:**
- Create: `crates/tagteam-engine/src/mappings.rs`, declared `pub mod mappings;` in `lib.rs`.
- Modify: `crates/tagteam-engine/src/refs.rs`. Extract `Engine::candidates`'s `Text` arm.
- Test: `crates/tagteam-engine/tests/config_mappings.rs` (new). Use `common::Fx` and write the
  file at `settings::config_path(&fx.env)`.

**Interfaces:**
- Consumes: `MappingEntry` and `Settings::load` (Task 1).
- Produces:
  - `pub enum MappedDir { Found(PathBuf), Missing(PathBuf) }`, deriving
    `Debug, Clone, PartialEq, Eq`, with `pub fn path(&self) -> &Path`.
    - `Found` holds the canonical directory, rebuilt from its components.
    - `Missing` holds the expanded path as given.
  - `pub fn mapped_dir(dir: &Path) -> MappedDir`. `fs::canonicalize` that succeeds on a directory
    gives `Found`. Anything else gives `Missing(dir)`. This is the one directory comparison every
    caller uses.
  - `pub enum MappingProblem { MissingDirectory, NoAccount, Ambiguous, ApiKey, Position,
    Conflict }`, deriving `Debug, Clone, Copy, PartialEq, Eq`.
    - `pub fn as_str(self) -> &'static str` gives the Global Constraints' strings.
    - `pub fn describe(self) -> &'static str` gives a phrase that completes "the mapping …". The
      run warning, the listing and doctor all use it. Wording is the implementer's, for example
      "names no account here" or "is a position, which a mapping never names".
  - `pub struct ConfigMapping { pub provider: ProviderId, pub entry: MappingEntry, pub dir:
    MappedDir, pub account: Option<AccountRow>, pub problem: Option<MappingProblem> }`.
    - `account` is set whenever the value names exactly one account, whatever the problem.
    - `problem` follows the precedence ruling.
  - `pub(crate) fn judge(store: Option<&Store>, p: &dyn Provider, entries: &[MappingEntry]) ->
    Result<Vec<ConfigMapping>, StoreError>`. Entries keep file order. Equivalent `Found` entries
    with different values are all `Conflict`.
  - `pub(crate) fn resolve_value(store: Option<&Store>, p: &dyn Provider, value: &str) ->
    Result<(Option<AccountRow>, Option<MappingProblem>), StoreError>`:
    - a `Position` parse gives `(None, Some(Position))`, with no store read;
    - an unparseable value, or no store, gives `NoAccount`;
    - otherwise it calls `text_candidates(store, text, Some(provider))`. Zero matches is
      `NoAccount`. Two or more is `Ambiguous`, with no account. One API-key account
      (`kind_traits(&row.kind).managed_key_axis`) is `(Some(row), Some(ApiKey))`.
  - `impl Engine`:
    - `pub fn config_mappings(&self, provider: &ProviderId) -> Result<Vec<ConfigMapping>,
      EngineError>` judges `Settings::load(&self.env, provider).0.mappings` against
      `existing_store()`.
    - `pub fn judge_value(&self, provider: &ProviderId, value: &str) -> Result<(Option<AccountRow>,
      Option<MappingProblem>), EngineError>`.
    - Neither creates anything.
  - `refs.rs`: `pub(crate) fn text_candidates(store: &Store, text: &str, provider:
    Option<&ProviderId>) -> Result<Vec<AccountRow>, StoreError>`. It is the alias-then-email
    branch, moved unchanged. `candidates` calls it.

- [ ] **Step 1: Write the failing tests** (`tests/config_mappings.rs`). Each entry's directory
  is created under `fx.env.home` unless the test says otherwise.
  - `alias_and_email_values_name_their_account`: `"~/w" = "work"`, where `work` is set with
    `fx.engine.set_alias`, and `"~/v" = "b@x.co"`. Both have `account` set to their account,
    `problem` `None`, and `dir` `Found(canonical)`.
  - `a_position_value_never_resolves`: `"~/w" = "2"`, with two accounts. The result is `account`
    `None`, `problem` `Some(Position)`.
  - `an_unknown_or_ambiguous_value_names_no_account`:
    - `"~/w" = "nobody"` gives `NoAccount`.
    - `"~/v" = "a@x.co"` gives `Ambiguous`, with `account` `None`. The two accounts sharing that
      email are inserted with `Store::insert_account`, with distinct `identity_key`s, as
      `common::add` builds them.
  - `an_api_key_account_is_named_but_does_not_apply`: `fx.add_api_key`, aliased `key`, and
    `"~/w" = "key"`. The result has `account` set, with `problem` `Some(ApiKey)`.
  - `a_missing_directory_is_reported_before_the_value`: `"~/gone" = "nobody"` gives
    `dir == Missing(home/gone)` and `problem == Some(MissingDirectory)`.
  - `a_key_naming_a_file_is_a_missing_directory`: `"~/f" = "work"`, where `~/f` is a file, gives
    `MissingDirectory`.
  - `equivalent_entries_conflict_only_when_their_values_differ`. `"~/w"` and the canonical
    absolute spelling of `~/w` are tried twice:
    - with values `"work"` and `"b@x.co"`, both entries are `Conflict`;
    - with `"work"` and `"work"`, neither has a problem.
  - `with_no_store_nothing_resolves_and_nothing_is_created`: a fresh `Fx` and `"~/w" = "work"`
    give `NoAccount`, and `fx.env.data_dir()` does not exist.
  - The existing `refs` callers stay green, which shows the extraction changed nothing.

- [ ] **Step 2: Run them to verify they fail.** `cargo test -p tagteam-engine --test
  config_mappings` fails to compile: there is no `tagteam_engine::mappings`.

- [ ] **Step 3: Implement** the Interfaces above. `judge` finds conflicts by grouping `Found`
  entries on their path.

- [ ] **Step 4: Run the tests to verify they pass**:
  `cargo test -p tagteam-engine --test config_mappings --test manage --test engine_basics`.

- [ ] **Step 5: Commit**: `Judge config.toml mappings against this machine's directories and
  accounts`.

---

### Task 3: `run`'s lookup

**Files:**
- Modify: `crates/tagteam-engine/src/run.rs`: `Mapped`, `mapped_account`, `plan_run`, and their
  doc comments.
- Test: `crates/tagteam-engine/tests/run_plan.rs`.

**Interfaces:**
- Consumes: `mapped_dir`, `resolve_value` and `MappingProblem::describe` (Task 2), and
  `Settings::load(&self.env, provider).0.mappings` (Task 1).
  - The file is read on every call, never from `self.settings()`, so a second plan sees the file
    as it stands now (ruling).
  - The load's warnings are dropped: the command's own load already printed them.
- Produces: `Mapped` gains `NotHere { why: String }`. `plan_run` turns it into `self.plain(req,
  launch, why, Some(format!("{why}; running plain {command}")))`, so `--require-session` refuses
  with `RequiresSession { why }`.

- [ ] **Step 1: Write the failing tests** (`run_plan.rs`)

  Add a helper, `map_in_file(fx, &[(key, value)])`, which writes
  `[provider.claude-code.mappings]` to `settings::config_path(&fx.env)`. No reload is needed,
  since `plan_run` reads the file itself. Then:
  - `a_second_plan_sees_the_file_as_it_stands`. Map `~/work` → a in the file, and plan from
    `~/work` on one engine: a's session. Rewrite the file to map `~/work` → c, and plan again on
    the same engine: c's session. Empty the file and plan once more: plain, with no warning.
  - `a_file_mapping_starts_a_session_by_alias_or_email`. An alias entry and an email entry each
    give `session(..)` of their account (a non-live one).
  - `the_deeper_mapping_wins_whichever_source_holds_it`:
    - file `~/work` → a and database `~/work/app` → c: from `~/work/app/src`, c wins;
    - database `~/work` → c and file `~/work/app` → a: a wins.
  - `the_file_wins_a_directory_both_sources_map`.
  - `a_trailing_slash_or_symlinked_key_maps_its_directory`. A key `"~/work/"` matches, and so
    does a key that is a symlink to `~/work`. Run from `~/work/app`.
  - `a_symlinked_cwd_finds_a_file_mapping`: the file maps `~/real`, and `cwd` is a symlink to it.
  - `a_missing_directory_is_skipped`: the file holds `"~/gone" = "a@x.co"`, the database maps
    `cwd` → c, and the result is c's session with no warning.
  - `a_file_mapping_that_names_no_account_runs_plain_claude_with_a_warning`. Run it for
    `"nobody"` and for an ambiguous email.
    - The warning contains the key as written, the config file's path, and
      `running plain claude`.
    - Under `requiring(..)`, the result is `Err(RequiresSession { why })`, and `why` names the
      key.
  - `a_position_value_wins_its_directory_and_runs_plain_claude`. The database maps `cwd` → a,
    and the file holds `"<cwd>" = "2"`. The result is plain, with a warning: no session of a, and
    none of position 2.
  - `equivalent_entries_with_different_values_run_plain_claude`. Both spellings of `cwd` are in
    the file, with different values, and the database also maps `cwd`. The result is plain, and
    the warning names both keys. With equal values, the account gets a session.
  - `a_file_mapping_to_an_api_key_account_runs_plain_claude_with_a_warning`. The existing
    `an_api_key_account_is_refused_named_or_mapped` stays as it is, for database rows.
  - `with_no_store_a_file_mapping_warns_and_creates_nothing`.
  - `a_corrupt_settings_file_leaves_database_mappings_working`. Write `not toml [` as the
    file, and map `cwd` → c in the database. The result is c's session.

- [ ] **Step 2: Run them to verify they fail.** `cargo test -p tagteam-engine --test run_plan`:
  the new tests fail, and every existing one passes.

- [ ] **Step 3: Implement `mapped_account`**
  1. Canonicalise `cwd` as today.
  2. The file candidates are the entries whose `mapped_dir` is `Found(p)` with
     `dir.starts_with(p)`. The deepest `p` wins (most components), and its group is every
     candidate with that `p`.
  3. The database candidate is `store.nearest_mapping` when a store exists.
  4. The file group wins when its depth ≥ the database row's.
  5. Values in the group differ: `NotHere`, naming every entry and the file. Otherwise,
     `resolve_value` runs once, for the group's value:
     - an account and no problem gives `Account`;
     - a problem gives `NotHere`.
  6. A database winner behaves as today.

  Only the winning group's value is resolved.

- [ ] **Step 4: Run the tests to verify they pass**: `cargo test -p tagteam-engine --test
  run_plan --test run_invariants --test run_shell`.

- [ ] **Step 5: Commit**: `Look up config.toml mappings in run, ahead of database rows for the
  same directory`.

---

### Task 4: Writing the file

**Files:**
- Modify: `crates/tagteam-engine/src/config.rs`.
- Test: `crates/tagteam-engine/tests/config.rs`. Follow its helpers for the file and the lock.

**Interfaces:**
- Consumes: `expand_mapping_key` (Task 1) and `mapped_dir` (Task 2).
- Produces:
  - `fn edit_file(&self, change: impl Fn(Option<(String, DocumentMut)>) -> Result<Option<String>,
    SettingsError>) -> Result<bool, EngineError>`. It is today's `edit` body made generic: the
    first look without the lock, the locked re-read, the `settings-read` hook point, and the
    atomic 0600 write. It returns whether the file changed. `edit` calls it, and keeps its own
    log line.
  - `pub fn mapping_key(home: &Path, dir: &Path) -> String`. `dir` is canonical.
    - The result is `~` or `~/rest` when `dir` is under `home`, or under
      `fs::canonicalize(home)`.
    - Otherwise it is `dir` as text.
  - `pub struct MappingWrite { pub written: String, pub changed: bool, pub path: PathBuf }`, deriving
    `Debug, Clone, PartialEq`. `written` is the entry's key as the file now holds it. `path` is
    the settings file as `config path` names it.
  - `impl Engine`:
    - `pub fn write_mapping(&self, provider: &ProviderId, dir: &Path, value: &str) ->
      Result<MappingWrite, EngineError>`.
      - `dir` is canonical.
      - An entry is equivalent when `mapped_dir(expand_mapping_key(key)).path() == dir`.
      - The first equivalent entry takes `value`, and keeps its key, its key's comments and its
        value's decor. Every later equivalent entry is removed.
      - With none, `mapping_key` is inserted at the end of the table. `table_at` makes the table
        when it is absent: `provider` and `provider.<id>` implicit, `mappings` explicit.
      - With the value already there and no other equivalent entry, nothing is written.
    - `pub fn remove_config_mappings(&self, provider: Option<&ProviderId>, dir: &Path) ->
      Result<usize, EngineError>`.
      - It removes every equivalent entry from `provider`'s table, or from every
        `provider.*.mappings` table in the file when `provider` is `None`.
      - A table left empty goes, then its parents, unless one holds a comment, as
        `remove_entry` does.
      - It returns the count. No file gives 0 and creates nothing, and so does no match.
  - Both writers log `tracing::info!(key = %format!("provider.{p}.mappings"), action = "map" |
    "unmap", "settings written")` once the lock is released. They never log the path or value.
  - A corrupt file is `SettingsError::Corrupt`, as `config set` reports it. A `mappings` that is
    not a table is `SettingsError::Invalid`.

- [ ] **Step 1: Write the failing tests** (`tests/config.rs`). Directories are created under
  `fx.env.home` and passed canonicalised.
  - `map_writes_a_home_relative_key_and_creates_the_table`: no file before. The text is exactly
    `[provider.claude-code.mappings]\n"~/Code/work" = "work"\n`, the mode is `0o600`,
    `changed` is true, and `written == "~/Code/work"`.
  - `map_writes_an_absolute_key_outside_the_home`: a directory under `fx.dir`, outside the home.
  - `map_writes_a_home_relative_key_when_the_home_is_a_symlink`. Build the `Fx` with an
    `env.home` that is a symlink to a real directory, using `Fx::with(.., adjust)`. A directory
    under it, passed canonical, gets a `~/…` key.
  - `map_rewrites_the_first_equivalent_entry_in_place_and_removes_the_others`. The file is
    `"[provider.claude-code.mappings]\n# work\n\"~/w\" = \"a\" # old\n\"/elsewhere\" = \"x\"\n\"<canonical home>/w\" = \"b\"\n"`.
    After writing `"c"`, the text is
    `"[provider.claude-code.mappings]\n# work\n\"~/w\" = \"c\" # old\n\"/elsewhere\" = \"x\"\n"`.
  - `map_of_the_value_already_there_writes_nothing`. `changed` is false, the file's mtime is
    unchanged, and `locks/config.lock` is not created when it was absent.
  - `unmap_removes_equivalent_entries_and_drops_an_empty_table`:
    `default_provider = "claude-code"\n\n[provider.claude-code.mappings]\n"~/w" = "a"\n` gives
    `default_provider = "claude-code"\n` and a count of 1.
  - `unmap_keeps_a_table_that_holds_a_comment`.
  - `unmap_without_a_provider_removes_every_provider_s_entries`: entries for `~/w` in the
    `claude-code` and `fake-agent` tables give a count of 2. With `Some(cc)`, only `cc`'s go.
  - `unmap_of_a_gone_directory_matches_its_expanded_path`: `"~/gone" = "a"`, and `dir` is
    `home/gone`, which does not exist. The count is 1.
  - `map_and_unmap_keep_an_inline_mappings_table_inline`.
    `[provider.claude-code]\nmappings = { "~/x" = "a" }\n`:
    - mapping `~/y` adds it inside the braces;
    - unmapping both leaves `[provider.claude-code]\n` and no `mappings`.
  - `a_corrupt_file_is_never_written`. Both writers return `Err(EngineError::Settings(
    SettingsError::Corrupt { .. }))`, and the bytes are unchanged.
  - `with_no_file_unmap_creates_nothing`. The count is 0, and neither the file nor the lock file
    exists.
  - The existing `config set`/`unset` tests stay green across the `edit_file` refactor.

- [ ] **Step 2: Run them to verify they fail.** `cargo test -p tagteam-engine --test config`:
  the new tests do not compile.

- [ ] **Step 3: Implement** the Interfaces above. Reuse `read`, `table_at`, `holds_comment`
  and `tidy`.

- [ ] **Step 4: Run the tests to verify they pass**: `cargo test -p tagteam-engine --test config
  --test settings --test storage_write`.

- [ ] **Step 5: Commit**: `Write and remove config.toml mappings under the settings lock`.

---

### Task 5: `doctor`

**Files:**
- Create: `crates/tagteam-engine/src/doctor/mappings.rs`. Declare `mod mappings;` in
  `doctor/mod.rs`, add the four `Id` constants beside the others, and call `run.mappings(p.as_ref())` in
  the per-provider loop, after `run.sessions`.
- Test: `crates/tagteam-engine/tests/doctor.rs`.

**Interfaces:**
- Consumes: `judge`, `MappingProblem::describe` (Task 2), and `Settings::load` (Task 1).
- Produces: `impl Run<'_> { pub(super) fn mappings(&mut self, p: &dyn Provider) }`. Its checks go
  under `Some(p.id())`, and every check names its fix.
  - It judges `Settings::load(env, &id).0.mappings` against `self.store_ref()`. When
    `all_accounts()` is `None` (store unusable), values are not judged. Pass the entries with no
    store, and keep only the `missing-directory` findings.
  - `mappings.directory`, info, one per entry whose directory is missing. It names the key and the
    file. The fix says to remove the entry by hand or with `tagteam unmap <path>`, if no machine
    uses it.
  - `mappings.value`, warn, one per entry with `no-account`, `ambiguous`, `api-key` or `position`.
    It names the key, the value, the file and `describe()`, and says `claude` runs plain there.
    The fix, `tagteam map ACCOUNT <path>`, writes a value that resolves.
  - `mappings.conflict`, warn, one per directory. It names every entry. The fix is to keep one
    entry, or run `tagteam map ACCOUNT <path>`.
  - `mappings.database`, info, only when the usable store holds n > 0 rows for `id`. The text
    says n directory mappings are still in the database. The fix: `tagteam map` moves each to
    `config.toml`.

- [ ] **Step 1: Write the failing test** (`tests/doctor.rs`):
  `doctor_reports_config_mappings_that_do_not_apply`. The file holds a missing directory, an
  unknown value, a position, and two conflicting entries. One database row is seeded. The
  report's `claude-code` checks hold exactly:
  - one `mappings.directory` (Info);
  - two `mappings.value` (Warn);
  - one `mappings.conflict` (Warn);
  - one `mappings.database` (Info).

  Each has a non-empty fix. A second report, after the file is emptied and the row removed,
  holds no `mappings.*` check.
- [ ] **Step 2: Run it to verify it fails**: `cargo test -p tagteam-engine --test doctor
  doctor_reports_config_mappings`.
- [ ] **Step 3: Implement** the Interfaces above.
- [ ] **Step 4: Run the tests to verify they pass**: `cargo test -p tagteam-engine --test
  doctor`.
- [ ] **Step 5: Commit**: `Report config.toml mappings that do not apply, and database ones left,
  in doctor`.

---

### Task 6: `map`, `unmap` and the listing

**Files:**
- Modify: `crates/tagteam/src/app/commands/mappings.rs` and `crates/tagteam/src/render.rs`
  (`mapping_json`, `mappings_json`, `mappings_human`).
- Modify: `crates/tagteam/tests/common/mod.rs`. Add `pub fn legacy_mapping(root: &Path, path:
  &str, id: &str)`, which seeds a database row with `Store::open_existing(..).set_mapping(path,
  claude-code, id, 1)`. Add `pub fn write_config(root: &Path, text: &str)`.
- Test: `crates/tagteam/tests/map_cli.rs`, plus every other test that used `map` to make a
  database row (Step 5).

**Interfaces:**
- Consumes:
  - `Engine::write_mapping`, `remove_config_mappings`, `MappingWrite` (Task 4);
  - `Engine::config_mappings`, `judge_value`, `ConfigMapping`, `MappingProblem` (Task 2);
  - `Engine::registry()` for the listing's providers.
- Produces:
  - `map ACCOUNT [PATH]`, in the ruled order:
    1. `resolve`, then `canonical_dir`.
    2. An API-key account is refused with `invalid-input`: it has no sessions to map.
    3. The value is `row.alias`, else `row.email`. Neither is refused with `invalid-input`,
       saying to give the account an alias first (`tagteam alias N NAME`).
    4. `judge_value(provider, value)` must give exactly this account's id. Otherwise the command
       is refused the same way.
    5. `write_mapping`, then `existing_store()` → `remove_mappings(dir, Some(provider))`. No store
       is created.
    - Human output: `Mapped {written} to {name} (position {n}) in {config path}.\n`.
    - JSON: today's `mapping` object with `addedAt` set to now, plus `"source": "config"`.
  - `unmap [PATH]`: `removed` = the file entries removed + the database rows removed. Human and
    JSON text are as today.
  - The listing, for `--provider`'s provider or every registered one:
    - The database rows, as today.
    - `config_mappings(p)` for each provider.
    - `render::mappings_json(rows, config)` gives `{"schemaVersion": 1, "mappings": [..as
      today..], "configMappings": [..]}`. Each `configMappings` object holds `path` (the
      `MappedDir` path), `written`, `value` and `provider`. It holds `account: {number, id,
      email, alias?}` when `account` is set, and `problem: as_str()` when `problem` is set.
    - `render::mappings_human(rows, config)` prints one line per entry from both sources,
      sorted by path, then provider. The provider column shows only when there are several, as
      today.
      - A file entry with an account: `{path}  {n}  {name}`.
      - A file entry without one: `{path}  -  "{value}"`.
      - A problem appends `  ({describe()})`.
      - A database row appends ``  (database; `tagteam map` moves it to the file)``.
      - With nothing from either source, the output is today's `NO_MAPPINGS`.

- [ ] **Step 1: Write the failing tests** (`map_cli.rs`). Existing tests that read `map`'s rows
  from `mappings` now read `configMappings`, or seed with `legacy_mapping`. The changes:
  - `map_stores_the_canonical_path_even_through_a_symlink`: `v["mapping"]["source"] ==
    "config"`, and `configMappings[0]["path"] == real`.
  - `map_defaults_to_the_current_directory_and_a_new_mapping_replaces_the_old`. The stdout is
    `Mapped ~/… to a@x.co (position 1) in <config path>.\n` (the directory is under `root/home`).
    After the second `map`, there is one `configMappings` entry with `value == "b@x.co"`.
  - `map_lists_every_mapping_in_path_order`. Seed `legacy_mapping` for `x`, and `map 2 y`.
    - `listed_shapes`, unchanged, gives exactly today's object for `x`.
    - The human output is
      ``{x}  1  a@x.co  (database; `tagteam map` moves it to the file)\n{y}  2  b@x.co\n``.
  - `unmap_removes_from_both_sources_and_counts_both`. Seed `x` in both sources:
    - `removed == 2`;
    - the config file no longer holds a `mappings` table;
    - `--provider claude-code` behaves the same.
  - `unmap_takes_the_listed_path_of_a_directory_that_is_gone`: the same, with the file entry.
  - `removing_an_account_removes_its_mappings`: seed with `legacy_mapping`. The assertions stay.
  - `on_a_fresh_machine_map_and_unmap_create_nothing`:
    - the JSON is `{"schemaVersion": 1, "mappings": [], "configMappings": []}`;
    - `map 1 /x` with no store fails `no-such-account`;
    - no config file is created, and no data directory.
  - `map_works_inside_a_run_shell_against_the_outer_home_s_store`: the entry lands in
    `Env::for_test(root)`'s config file.
  - New, `map_writes_the_alias_else_the_email`. After `alias 1 work`, `map 1 x` writes
    `"~/x" = "work"`, and `map 2 y` writes `"~/y" = "b@x.co"`.
  - New, `map_refuses_an_api_key_account_an_ambiguous_email_and_an_account_with_neither`.
    - The API-key account comes from `add-token`.
    - The ambiguous email comes from two `login`+`add` runs of `a@x.co`, with orgs `o1` and `o2`.
    - The account with neither alias nor email is seeded with `Store::insert_account`, with an
      identity that has no email.
    - Each refusal is `invalid-input`, and the config file is unchanged.
  - New, `map_deletes_the_database_row_for_the_directory`. Seed `legacy_mapping(x)`, then
    `map 2 x`: `mappings` is empty, and `configMappings` has one entry.
  - New, `the_listing_reports_every_problem`. Write a file holding one entry for each `problem`
    value. The `configMappings` problem set is exactly the six strings. The human output holds
    `-  "nobody"` for the `no-account` entry.

- [ ] **Step 2: Run them to verify they fail.** `cargo test -p tagteam --features test-support
  --test map_cli`.

- [ ] **Step 3: Implement** the Interfaces above.

- [ ] **Step 4: Run the tests to verify they pass**: `cargo test -p tagteam --features
  test-support --test map_cli`.

- [ ] **Step 5: Bring the other `map` users along.** Run `cargo test -p tagteam --features
  test-support --test run_cli --test run_shell_cli --test app` unsandboxed, and fix each failure
  from `map` now writing the file. Keep each test's intent.
  - `a_mapped_target_removed_before_the_launch_reads_it_runs_plain_claude_after_one_more_plan`:
    after `remove 1`, the file entry names no account. The second plan still runs plain `claude`
    with one warning, which now holds `names no account`, not `was removed`. Update its comment.
  - `a_target_that_changes_again_under_the_second_plan_s_launch_is_the_command_s_error`. Its
    `unmap` of `work/app` now removes the file entry, and the second plan must see that,
    through Task 3's fresh read. It passes with its assertions unchanged.
  - Report each test changed, and why, in the task report.

- [ ] **Step 6: Commit**: `Map and unmap in config.toml, and list both sources`.

---

### Task 7: `alias` and `remove` warnings

**Files:**
- Modify: `crates/tagteam/src/app/commands/accounts.rs` (`remove`, `alias`'s set and unset arms).
  The diff helper goes in `crates/tagteam/src/app/commands/mappings.rs`, beside the listing.
- Test: `crates/tagteam/tests/map_cli.rs`.

**Interfaces:**
- Consumes: `Engine::config_mappings` and `ConfigMapping` (Task 2).
- Produces: `pub(in crate::app) fn mapping_warnings(before: &[ConfigMapping], after:
  &[ConfigMapping], file: &Path) -> Vec<String>`.
  - Entries are matched by `(written, value)`.
  - Each entry whose `before.account` was `Some(a)` warns when `after`'s account id differs, or is
    gone.
  - The warning names the key, the value and the file, and what the entry names now:
    - no account: "`claude` runs plain there, with a warning";
    - another account: its position and name.
  - `remove` and `alias` take `before` after `resolve` and `after` once the engine call returns,
    both for `row.provider`. Each warning goes through `self.io.warn`. A failed
    `config_mappings` read gives no warnings, and the command goes on.

- [ ] **Step 1: Write the failing tests** (`map_cli.rs`)
  - `removing_an_account_warns_about_config_mappings_that_named_it`. The file holds
    `"~/x" = "a@x.co"`, then `remove 1`:
    - stderr holds `~/x`, `a@x.co` and the config path;
    - the file's bytes are unchanged;
    - `removing_an_account_removes_its_mappings` still passes.
  - `unsetting_or_renaming_an_alias_warns_about_config_mappings_that_used_it`. Run `alias 1 work`,
    and write `"~/x" = "work"`.
    - `alias 1 home` warns, naming `~/x`.
    - `alias 1 work` restores it, with no warning, since the entry named no account before.
    - `alias 1 --unset` warns.
    - The file's bytes are unchanged throughout.
  - `an_alias_change_no_entry_used_says_nothing`: `alias 2 other` with the same file gives empty
    stderr.
- [ ] **Step 2: Run them to verify they fail**: `cargo test -p tagteam --features test-support
  --test map_cli warn`.
- [ ] **Step 3: Implement** the Interfaces above.
- [ ] **Step 4: Run the tests to verify they pass**: `cargo test -p tagteam --features
  test-support --test map_cli --test cli`.
- [ ] **Step 5: Commit**: `Warn when alias or remove changes what a config.toml mapping names`.

---

### Task 8: End to end and the records

**Files:**
- Test: `crates/tagteam/tests/run_cli.rs`. Use its `Home` (two accounts; the fake `claude` in
  `<root>/bin`, recording into `FAKE_CLAUDE_OUT`; `<root>/work`), `session(out)` and
  `Home::profile()`.
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`, `README.md`,
  `CHANGELOG.md`.

- [ ] **Step 1: Write the end-to-end test.**
  `the_wrapper_in_a_directory_mapped_in_config_toml_starts_a_session`. It must go through the
  wrapper; there is no fallback to calling `tagteam run` directly. Setup:
  - The settings file holds `"<root>/work" = "a@x.co"`, under
    `[provider.claude-code.mappings]`. Account a is not the live login.
  - Run `bash -c '<shell-init bash output>\nclaude x'` from `<root>/work`:
    - with `std_cmd`'s environment;
    - with `FAKE_CLAUDE_OUT` set;
    - with a `PATH` that puts `<root>/bin` (the fake `claude`) and the directory of
      `assert_cmd::cargo::cargo_bin("tagteam")` before the inherited `PATH`.

  Expected:
  - the shell exits 0;
  - `session(&out)` is `Some`;
  - its `CLAUDE_CONFIG_DIR` is `Home::profile()` (a's).

  When `bash` is not installed, use `zsh -f` with `shell-init zsh`. When neither is, the test
  fails, naming both.
- [ ] **Step 2: Run it**, unsandboxed: `cargo test -p tagteam --features test-support --test
  run_cli the_wrapper_in`. It passes on Tasks 3 and 6. Then check that it fails without the
  feature:
  1. Revert Task 3's `run.rs` change locally.
  2. Run the test, and see it fail.
  3. Restore the change.
- [ ] **Step 3: The core spec.** Each passage ends with "*(Amended by the config-mappings
  spec.)*", linking the spec:
  - §12.7: mappings live in `config.toml`, the database is a legacy source, and the lookup and
    precedence are S§3.2's;
  - §6.4: the `provider.<id>.mappings` table, next to the price tables' note;
  - §6.1: the `mappings` table's comment says it is legacy, read only by `run` and `map`'s
    listing, and deleted per directory by `map` and `unmap`;
  - §10.3: under `remove` and `alias`, the cascade covers database rows, plus S§3.6's warnings.
- [ ] **Step 4: README, "Running accounts side by side."**
  - The `config.toml` form, with S§3.1's example.
  - The dotfile-manager note: `map` writes the deployed file, so edit the source, or re-add the
    file after `map`.
  - The IDE extensions and the desktop app do not go through the wrapper. The desktop app's Code
    tab uses its own sign-in.
- [ ] **Step 5: CHANGELOG `[Unreleased]`.**
  - Added: mappings in `config.toml`, the `doctor` checks, and the `alias`/`remove` warnings.
  - Changed:
    - `map` writes `config.toml` and moves a database mapping there;
    - `map --json` lists file entries under `configMappings`;
    - a script reading `mappings` sees an entry leave it when `map` moves the entry (S§3.4);
    - `map` refuses API-key accounts.
- [ ] **Step 6: Commit**: `Record config.toml mappings in the core spec, README and CHANGELOG`.

---

### Task 9: Verification (controller)

- [ ] **Step 1.** Run `cargo fmt --all --check` and the three clippy runs. Then run
  `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`
  unsandboxed. Expected: all pass. Report passed/total.
- [ ] **Step 2.** `git diff --stat main -- Cargo.lock` is empty, so cargo-deny has nothing new to
  check. If it is not empty, run cargo-deny unsandboxed, as the brief gives it.
- [ ] **Step 3.** Spec coverage. Walk S§4's test list against the tests named above, and list
  any gap in the PR body.
