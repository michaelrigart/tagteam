# tagteam — review refactor: the provider boundary and clean code

**Status:** In progress — https://github.com/michaelrigart/tagteam/pull/22 (R1), https://github.com/michaelrigart/tagteam/pull/24 (R2)
**Date:** 2026-10-19
**Scope:** Fixes the architecture and clean-code findings of the 2026-10-19 codebase review, plus
its PERF-R-1, on branch `review-refactor`. Every finding the review raised on that axis is
assigned to a pull request here, held for the TUI redesign (§10), or dropped with a reason (§13).
**Builds on:**
- **Core spec:** [`2026-09-26-tagteam-core-cli-design.md`](2026-09-26-tagteam-core-cli-design.md)
  (Implemented), cited as "core §n". It stays the authority for behaviour. This spec changes
  structure, and changes behaviour only where §11 lists it.
- **Daemon spec:** [`2026-10-14-tagteam-daemon-design.md`](2026-10-14-tagteam-daemon-design.md),
  cited as "D*n*".
- **TUI spec:** [`2026-10-14-tagteam-tui-design.md`](2026-10-14-tagteam-tui-design.md), cited as
  "TUI §n".
- **Token report spec:** [`2026-10-14-tagteam-token-report-design.md`](2026-10-14-tagteam-token-report-design.md),
  cited as "TR §n".

**Review baseline.** The review ran read-only on `main` at `683b266`, the base of this branch.
Its reports sit in the main checkout's gitignored `.superpowers/research/codebase-review-2026-10-19/`,
so they are not part of this record. Finding IDs (ARCH-n, CC-E-n, CC-O-n, PERF-R-n) are theirs.
Each finding was re-verified against `683b266` before this spec was written. The facts this spec
relies on are restated here. Line numbers are as of `683b266`, and will drift.

**Parallel work.**
- Branch `review-hardening` carries the review's security and performance findings, and merges
  first (§3.2).
- A TUI UI/UX review has started. A redesign will likely take over `crates/tagteam-tui`,
  `crates/tagteam/src/tui_host.rs` and `crates/tagteam/src/tui_cmd.rs`, so this spec holds its
  TUI-side items for that work (§3.4, §10).

---

## 1. Purpose

### 1.1 Why

The review found no Critical issue and one High one, ARCH-1. Claude Code knowledge has leaked out
of `tagteam-cc` into `tagteam-provider`, `tagteam-core`, the engine, the CLI and the TUI. That
breaks core §1.2 ("providers can be added without reshaping the store, the engine or the CLI") and
core §4.1 ("nothing in `tagteam-engine` names a CC path, key or endpoint"; "a future provider is
one new crate plus one line in the registry"). The core spec names Codex as the next provider.
Two practical effects follow:
- Claude Code drift (core R2) lands in "neutral" code, so the weekly compat job's failures point
  at the wrong crate.
- A second provider would have to edit `Env`, the provider crate's liveness parser, engine error
  texts, the service unit, the CLI renderer, `app.rs` and the statusline.

The `FakeAgent` guard cannot see any of this. Its session records use Claude Code's format, and
it never runs through the CLI, the statusline or service units.

The rest is maintainability debt that is cheapest to pay before the next frontend (the menu bar)
and the next provider arrive:
- policy copied across modules, where some copies have already diverged;
- closed vocabularies passed around as strings;
- three modules of 2.5k to 3.3k lines.

### 1.2 Goals

1. **The provider boundary holds, and a test keeps it.**
   - No production code in `tagteam-provider`, `tagteam-core`, `tagteam-engine` or
     `tagteam-daemon` names Claude Code, its paths, keys, environment variables or file formats.
   - `tagteam-tui` is held to the same rule, except for two allowlisted lines that the TUI
     redesign removes (§10).
   - In the binary, only the composition root names `tagteam_cc`.
   - A test fails if any of that regresses (§5.9).
2. **One composition root.** The statusline and completions use the same registry as every other
   command (ARCH-2).
3. **Each policy lives in one place.** That covers the switchable-account rule, the refresh-gate
   outcome table, the never-delete-the-live-login guard, the CLI's engine bootstrap and its error
   reporting.
4. **Closed sets are types.** That covers failure kinds, event sources and kinds, settings keys,
   and the daemon protocol's modes, outcomes and events, which get typed accessors any client
   can use.
5. **No module mixes unrelated responsibilities.** `engine/switch.rs`, `engine/doctor.rs` and
   `tagteam/src/app.rs` become module directories along their existing seams.
6. **A manual switch makes fewer Keychain calls**, and a test pins the count (PERF-R-1).

### 1.3 Non-goals

- **Splitting the `Provider` trait into sub-traits** (ARCH-9, CC-O-6). Core R7 assigns that to the
  first real second-provider spec.
- **The TUI crate, `tui_host.rs` and `tui_cmd.rs`.** They are held for the TUI UI/UX review (§10).
  This branch edits them only where a changed engine, protocol or CLI API would otherwise stop
  them compiling (§3.4).
- **Moving `doctor`'s daemon checks into `tagteam-daemon`** (ARCH-6 relocation). It changes report
  order and needs a doctor-composition design. Do it when the menu bar or the next daemon check
  lands.
- **Newtypes across the store**: `Seconds`/`Millis`, `IdentityKey`, `Fingerprint`, `Alias`,
  `CredKind`, and `org_uuid` as an `Option` (CC-E-8, CC-E-12, CC-O-14). The scoped helpers in §8
  are the part done here.
- **Anything `review-hardening` owns** (§3.2).
- **A second real provider.** This spec makes one possible; it does not add one.

### 1.4 Anti-goals: failure even if the code compiles

- **Claude Code's user-visible output changes** beyond §11's list. Every message, JSON field,
  exit code and help line not listed there stays byte-identical. The existing tests that assert
  text are the check.
- **A wire change.** The daemon protocol's JSON, `--json` schemas (other than §11's error kinds)
  and the store schema do not change. There is no store migration.
- **A weaker safety invariant.** The lock order, the journal, the live-login guard, `Read<T>`
  tri-states and the §9.1 freshness rule for live-credential reads stay as strict as today.
- **A trait that grows a method per leak.** Provider-specific facts go into a few data structs
  (§5.2), not a dozen new methods.
- **Churn without a finding behind it.** Renames and moves are limited to what §5 to §9 name.
- **Pre-empting the TUI redesign.** No TUI behaviour or structure changes here (§3.4).

## 2. Decisions

**Michael's rulings (2026-10-19):**

| # | Question | Ruling |
|---|---|---|
| D-1 | ARCH-1: restore the provider boundary, or relax core §1.2/§4.1? | **Restore it.** R1 and R5 below |
| D-2 | ARCH-7/CC-O-20: does the TUI keep driving the CLI through argv and captured text? | **Keep argv, and fix its two string couplings**: a typed unlock prompt, and warnings passed in rather than stripped from captured stderr. Typed calls wait for the menu-bar spec. The fix touches `tui_host.rs`, so it is handed to the TUI redesign (§10) |
| D-3 | CC-E-3: are the `--json` `error.type` values `io` and `invalid-input` a stable interface? | **Kind values are stable, but the mapping can be corrected.** No kind is ever renamed or removed. Which kind a given refusal reports may be corrected, as an additive change noted in `CHANGELOG.md` (§8.5, §11) |

**Calls made while drafting** (Michael checks these at sign-off):

| # | Call | Why |
|---|---|---|
| C-1 | ARCH-4 takes the narrow step: Engine query methods and lock tokens on three store mutators. The `Store` and its mutators stay `pub` | No crate other than the engine uses those mutators. Making them `pub(crate)` behind a `test-api` feature would touch all 55 engine test binaries for no extra protection |
| C-2 | The rescue-unreadable message `tagteam run` prints names the damaged files, as `switch`'s already does | One unreadable rescue currently gives two different messages |
| C-3 | `Settings.default_provider` becomes `Option<ProviderId>`, falling back to the registry's first provider. `config get`/`list` still print the effective id | The core crate stops naming `claude-code`, and `Settings::load`'s signature does not change |
| C-4 | Clap help that describes provider-generic behaviour gets neutral wording. Help for a Claude-Code-only feature (the statusline, the shell wrapper) may keep naming it | Help is documentation, not structure. Building help at runtime from the registry is not worth its complexity with one provider |
| C-5 | The neutrality test is `tagteam-fake/tests/neutrality.rs`, not an xtask lint | It runs in the existing `cargo test --workspace`, with no workflow edit. CI workflows belong to hardening |
| C-6 | `core/merge.rs` stays in core, unparameterised | Only providers call it, as an optional helper (`tagteam-cc` and `tagteam-fake`). The engine never does, so it does not breach §4.1 |
| C-7 | `BLOCK_MS` (5 h, `core/tokens/blocks.rs`) and the statusline placeholders `{5h}`/`{7d}` stay | They belong to the normalised usage-window model (core §8.2) and the token report's unit (TR). Revisit them at the second-provider spec |
| C-8 | `AutoEvent::NoSwitch.reason` stays a `String` and gains `NoSwitchReason::parse` | Retyping it touches 14 sites, among them `daemon/notify.rs` (hardening) |
| C-9 | Settings messages: `SettingsError::UnknownProvider` gets the registry's ids from `config.rs:119`, its only producer, and keeps its text ("…such as claude-code"). `KeyKind::Provider`'s static reason drops its example | One example needs the registry, which the static reason cannot reach. The changed text is listed in §11 |
| C-10 | `alias` keeps its runtime usage check (`ALIAS_USAGE`) instead of a clap `ArgGroup` | An `ArgGroup` would replace tagteam's usage line with clap's own error text, a user-visible change nothing pins |
| C-11 | The daemon protocol's typing is additive: typed accessors and predicates beside the existing `String` fields, which stay | The TUI reads those fields and is held (§3.4). The redesign adopts the accessors, and the fields can then be retyped |

## 3. Constraints

### 3.1 Verification

- Every PR passes the CI commands in `CONTRIBUTING.md`:
  - `cargo fmt --all --check`;
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`, with and without
    `--features tagteam/test-support`;
  - `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support`.
- Tests run on macOS and, via the Linux Docker repro, on Linux before each PR is reported done.
- Pure moves are their own commits, so a reviewer can read them with `git diff --color-moved`.
  No behaviour edit may go into a move commit.
- Test hook-point names (`hooks::point("…")`) and the `crate::switch::…`/`crate::doctor::…` paths
  other modules import survive every move. Re-export them from the new `mod.rs`.

### 3.2 `review-hardening`

**Ownership.** Hardening's spec is
`docs/superpowers/specs/2026-10-19-tagteam-review-hardening-design.md` on branch
`review-hardening` (cited as "H*n*"). Its H1.2 leaves `switch.rs`, `launch.rs`, `doctor.rs`,
`app.rs` and the daemon's switchable-account copies to this branch. It touches only two of them:
one line in `app.rs` (H11.2), and possibly the arguments of `switch.rs`'s one `vault.store` call
(H5.3).

**Files hardening owns**, which this branch does not edit except where marked:
- **`tagteam-provider`:** `security.rs`, `process.rs` and `keychain.rs`, plus the provider's test
  doubles: `FakeKeychain`, `FakeClock`, `ScriptedHttp`, `FakeProcessProbe`, `ScriptedSpawner` and
  `Env::for_test`'s gating (CC-O-4, ARCH-11, CC-O-5).
- **`tagteam-engine`:** `refresh.rs`, `store/tokens.rs` and `tokens/*`.
- **The daemon's wake-up code:** the loops in `server.rs`, `notify.rs` and `scheduler.rs`.
- **CI:** `.github/workflows/*` and `dist-workspace.toml`.

**Files both branches edit.** These are not hardening's, but hardening changes them:
- **What hardening changes:**
  - **`engine/vault.rs`:** H5.3 rewrites `Vault::store` (the vault form, and a read-back of it),
    and adds a `vault_form` provider hook.
  - **`engine/collect.rs`:** H4 makes the Keychain read inside collection a cancellation point.
  - **`tagteam-cc/src/live.rs` and `shape.rs`:** H11.3 adds a mode check before splicing
    `primaryApiKey`; H5.3 changes `--full` exports.
  - **The rest:** `prompt.rs` (H11.1); `render.rs`, `statusline.rs` and `tokens_cmd.rs`
    (H11.4, a control-character helper in `tagteam-core`); the daemon's `client.rs` (H11.5);
    `run.rs` (H9).
- **The rule:**
  - Where this branch's change to such a file is textual (R1, R2, R3), it proceeds and is
    rebased.
  - Where it depends on what hardening changes, it starts only after hardening has merged.
    That covers all of R4, which needs `vault.rs`, `collect.rs` and `refresh.rs`, and its
    PERF-R-1 part (§8.11), which needs `vault.rs`, `cc/live.rs` and the vault form. It also
    covers all of R5, which needs `liveness.rs` and `tokens/evidence.rs`.

**Edits to hardening's files**, made in R4 and R5 after the rebase:
- `refresh.rs`:
  - the two `"cli"` event sources at `:352` and `:405`, and their test pin at `:836` (§8.7);
  - the gate's failure-token writers (§8.6);
  - the quarantine-reason fallback at `:402` (§8.9).
- `tokens/evidence.rs`, about 8 lines, and the parser region of `liveness.rs` (R5).

One exception does not wait: `tagteam-provider/src/lib.rs` re-exports both the fakes and the
liveness parser. R1 adds its `Platform` re-export on a line of its own, to keep the conflict
trivial.

**Before R2 and R3 start**, check hardening's diff for `switch.rs`, `doctor.rs` and `app.rs`. If
it touched them, redo the mechanical move commit on top of the rebased base; do not merge it by
hand.

### 3.3 Size

GitHub's "Rebase and merge" refuses large branches; one with 141 commits was refused. The xreview
pre-merge gate can carry only about 100 KB of inline diff. Hence §4's split into five PRs, each
reviewed by Codex in slices where it exceeds that.

### 3.4 The TUI hold

**Held files:** `crates/tagteam-tui/**`, `crates/tagteam/src/tui_host.rs` and
`crates/tagteam/src/tui_cmd.rs`. This branch edits them only to keep them compiling against an
engine, protocol or CLI API this spec changes, and then minimally:
- no renames inside them;
- no restructuring;
- no behaviour or text change.

**Changed APIs** that a held file calls keep a source-compatible form where that is cheap: a thin
wrapper under the old name, or additive accessors (C-11). The planner lists each held-file edit
in the plan.

## 4. Delivery

Five stacked Draft PRs, in this order. Each has its own plan, written just in time
(`docs/superpowers/plans/2026-10-xx-tagteam-refactor-rN-<topic>.md`), and is reviewed by Codex
before execution starts. Work runs in the `review-refactor` worktree. Each PR is opened from a
branch ref that points at its last commit, so Michael can merge R1 while R2 runs.

| PR | Topic | Depends on | Waits for hardening |
|---|---|---|---|
| R1 | Provider boundary, step 1, and the composition root (§5) | — | no |
| R2 | Engine structure: the switch and doctor splits, the guard API (§6) | R1 | no |
| R3 | CLI and the daemon protocol's typed values (§7) | R1 | no |
| R4 | Engine contracts: policy dedup, typed vocabularies, error kinds, PERF-R-1 (§8) | R2 | yes |
| R5 | Provider boundary, step 2: session records (§9) | R1 | yes |

**Merge order.**
- R1 to R3 run while hardening is still open, and are rebased onto it when it merges.
- R4 and R5 start on the rebased stack.
- Hardening merges before R1, as both specs say.

**Each PR:**
- fills `.github/pull_request_template.md`;
- adds a `CHANGELOG.md` entry under `Unreleased` for each user-visible change (§11);
- records its review rulings under "Rulings" in the PR description.

**Status.** The spec moves to `In progress` with the first PR, and to `Implemented` (citing every
PR) in R5's final pre-merge commit. Items held for the TUI redesign do not block `Implemented`;
§10 records where they went.

## 5. R1 — Provider boundary, step 1, and the composition root

ARCH-1 (all but session records and the TUI's wording), ARCH-2, the cc part of ARCH-12, CC-O-21,
and the `statusline` part of CC-O-22.

### 5.1 `Env`

**Today.** `Env` (`tagteam-provider/src/env.rs:55,57`) has two dedicated fields,
`claude_config_dir` and `claude_securestorage_config_dir`. `Env::from_process` reads them
straight from `CLAUDE_CONFIG_DIR` and `CLAUDE_SECURESTORAGE_CONFIG_DIR` (`:103-104`). Beside
them sits `Env.vars` (`:61-63`), a generic map of captured variables. The two hold different
things, and both distinctions matter:
- **The dedicated fields are the effective values** that cc resolves its homes and Keychain
  service from (`cc/naming.rs:17-21`, `cc/provider.rs:619`, `statusline.rs:288`).
  - `None` means undefined, and `Some("")` means defined but empty.
  - `apply_outer_home` (`cc/session.rs:118-138`) restores them unconditionally from the run
    shell's outer record.
  - `session_env` and its siblings (`cc/session.rs:151-163`) overwrite them for a profile.
- **`vars` holds raw captured values and presence markers.** `app.rs:244-264` puts the session
  variable, the variables a session scrubs and the service unit's recorded variables in it. A
  scrubbed variable the process has is recorded as an empty-string marker. Readers:
  - `launch.rs:215` reads presence for the environment warnings;
  - `recorded_env` reads values for service units and doctor.
  - `apply_outer_home` rewrites a name in `vars` only when `vars` already tracks it.

**Change: generalise the fields; leave `vars` alone.**
- Replace the two fields with one neutral map, `Env.homes: BTreeMap<String, OsString>`.
  - It holds the effective values of the variables a provider locates its homes with.
  - An absent key means undefined, and `""` means defined but empty. That carries exactly the
    `Option<OsString>` distinction.
- `ProviderVars` (§5.2) gains `homes: &'static [&'static str]`; Claude Code's are the two
  variables. The binary fills `Env.homes` from the process for the union of those names, at the
  boundary where it captures `vars` (`app.rs:247-249`). `Env::from_process` stops naming them.
- `tagteam-cc` gains accessors that read and write `Env.homes`, for example
  `tagteam_cc::env::config_dir(&Env)`, `securestorage_dir(&Env)` and `set_config_dir(&mut Env,
  …)`. Every current reader and writer of the two fields moves to them: cc's naming, provider and
  session code, the statusline, xtask's `compat/guard.rs:80-81`, and about 126 test lines.
- **`apply_outer_home` keeps both of today's rules.**
  - It restores `homes` unconditionally from the outer record, as it restores the fields today.
  - It rewrites `vars` only for names `vars` already tracks.
- `session_env` and its siblings write `homes` where they write the fields today.
- **`vars`, its presence markers and their readers do not change.** Once the effective values
  live in `homes`, a marker in `vars` is never read as a value, which was the only reason to move
  the markers.
- **Tests.**
  - A test pins `apply_outer_home` for both variables, for each combination of outer defined,
    outer defined empty and outer undefined, against inner tracked and inner untracked. It
    covers both the effective value (`homes`) and the presence that `launch.rs:215` reads.
  - The existing `naming.rs` tests, which cover the Keychain service name, must pass unchanged
    apart from the accessor spelling.
- `Env`'s `Debug` and `for_test` follow. `Debug` prints `homes`' names and values as it prints
  the two fields today. `for_test`'s gating belongs to hardening, but its body may change.

### 5.2 Provider-supplied data

Today these facts are hard-coded outside cc:
- the binary's captured variables: `CLAUDECODE` and `CLAUDE_CODE_SESSION_ID` (`app.rs:145-149`,
  `:294-295`, `:974`);
- the service unit's recorded variables (`engine/service.rs:36-37`, `RECORDED_VARS`);
- the post-switch hints (`render.rs:503-512`);
- the supervisor wording ("Claude Code background daemon", "`claude daemon stop --any`",
  `daemon.lock`) in `engine/error.rs:296-302`, `:359-362` and `doctor.rs:2537`.

Add two static data structs to the trait. Field names are a shape; the builder may adjust them.

```rust
pub struct ProviderVars {
    /// Variables whose effective values locate the provider's homes; kept in `Env.homes` (§5.1).
    pub homes: &'static [&'static str],
    /// Variables the binary captures into `Env.vars` at start-up.
    pub captured: &'static [&'static str],
    /// Variables a service unit records (D9.1), in addition to tagteam's own.
    pub unit: &'static [&'static str],
    /// The variable a running session exports with its session id, if any.
    pub session_id: Option<&'static str>,
}
pub struct ProviderWords {
    pub after_switch_file: &'static str,
    pub after_switch_keychain: &'static str,
    pub after_switch_managed_key: &'static str,
    /// What the provider's background supervisor is called, and how to stop it.
    pub supervisor: Option<&'static str>,
    pub supervisor_stop: Option<&'static str>,
}
trait Provider {
    fn vars(&self) -> &'static ProviderVars;
    fn words(&self) -> &'static ProviderWords;
    /// Where and what to write so the agent shows tagteam's statusline; `None` if unsupported.
    fn statusline_setup(&self, env: &Env, command: &str) -> Option<(PathBuf, String)> { None }
    /// An export format of another tool that this provider can import (§5.6).
    fn foreign_export(&self) -> Option<&'static ForeignFormat> { None }
}
```

That makes four new methods. The rest comes from what the trait already has: `display_name()`,
`launch_command()`, `session_dir_var()`, `supervisor_lock()`, `kind_traits()` and
`share_policy()`. Hardening adds `vault_form` (H5.3) beside them; the two additions conflict only
textually.

**Users of this data:**
- **The binary's capture list** is the union of `vars().captured` across the registry. It
  replaces `session_vars` (`app.rs:284-297`) and the hard-coded `CLAUDE_CODE_SESSION_ID` read at
  `app.rs:974`.
- **`Engine::recorded_vars()`** returns XDG, then `∪ vars().unit`, then the proxy variables,
  `TAGTEAM_LOG` and `PATH`, in today's order. `recorded_env` takes the list. Callers:
  `service.rs:152`, `doctor.rs:1315`, and `app.rs:263` and `:1785`.
- **`SwitchOutcome` gains `activation_hint: Option<&'static str>`.** The engine chooses it from
  `words()` and `kind_traits(kind).managed_key_axis` (`switch.rs:129-144`). `render.rs` stops
  importing `tagteam_cc::shape::KIND_API_KEY` (`render.rs:2`, `:507`).

### 5.3 Messages

**Engine and CLI messages** that name Claude Code or `claude` are built from `display_name()`,
`launch_command()` and `words()`, so Claude Code's output stays byte-identical. Known sites:
- `engine/error.rs:49` and `:72` become `NoLiveLogin { launch }` and `NeedsRelogin { …, launch }`.
- `error.rs:296-302`, `:359-362` and `:379`.
- `collect.rs:1124`, `export.rs:32`, `doctor.rs:2537` and `:3233`.
- `app.rs:115` and `:356`, `render.rs:21` (`NO_ACCOUNTS`), `statusline.rs:196`.
- `tokens_cmd.rs:263`.

**`SessionOwner::of(state, p)`** takes the provider so its text can name the supervisor. Six call
sites change.

**Settings messages (C-9).**
- `SettingsError::UnknownProvider` (`settings.rs:761`) carries the registry's ids, supplied at
  `config.rs:119`, and keeps its current text.
- `KeyKind::Provider`'s reason (`settings.rs:341`) drops its "such as \"claude-code\"" example.
  `engine/tests/settings.rs:1153` pins that text and changes with it (§11).

**The TUI's two strings** (`screens/accounts.rs:25`, "Log in with claude"; `actions.rs:291`,
"Claude Code is not changed") are held (§3.4). §5.9 allowlists them, and §10 hands them to the
redesign.

**Tests that must be edited:**
- `engine/tests/add.rs` (the variant's shape);
- `engine/tests/settings.rs:1153`;
- `render.rs`'s unit tests;
- the `error.rs` and `mkdir_lock.rs` unit tests that build `Timeout`.

Every other test that asserts message text must pass unchanged. The PR lists which ones the
builder confirmed.

### 5.4 Identity, platform and locks

- **The provider id.** `CLAUDE_CODE` moves from `tagteam-core/src/ids.rs:3` to
  `tagteam_cc::CLAUDE_CODE`, under the same name. Its 44 users outside core and cc are tests, and
  each changes one import. Settings fall back per C-3 (`settings.rs:18`, `:825`); the binary's
  fallback at `app.rs:349` uses the registry.
- **`Platform`** moves from `tagteam_cc::live` to a new `tagteam-provider/src/platform.rs`. It also
  picks the notifier and the service manager (`app.rs:695`, `:813-818`, `:1731`, `:1883`), so it
  is an OS type. `tagteam-cc` keeps `pub use` at `live.rs:161` so its 30 test files compile
  unchanged.
- **Lock wording.** `MkdirLockSpec` gains `role: &'static str` and
  `timeout_hint: Option<&'static str>`. cc sets both at its seven constructors (`cc/locks.rs`),
  which removes CC's lock-file names and stale-lock text from `mkdir_lock.rs:24-42` and
  `:138-149`.
  - `LockError::Timeout(PathBuf)` becomes `Timeout(TimedOut { path, hint })`, with
    `From<PathBuf>`. `Timeout(_)` patterns keep compiling, including the engine's
    `tokens/mod.rs:40`.
- **`ProviderError::ShadowingItem`** becomes `{ item: String, agent: &'static str }`
  (`provider.rs:372-375`). It no longer says "Claude Code" itself.
- **`format_iso8601`** is imported from `tagteam_core::time` in `displaced_cmd.rs`, `history.rs`,
  `logging.rs` and `render.rs`. The re-export at `cc/usage.rs:15-17` goes if nothing else uses it.

### 5.5 One composition root (ARCH-2)

- **One registry function.**
  `app::registry(keychain: Arc<dyn Keychain>, platform: Platform, api_base: Option<&str>)
  -> ProviderRegistry` is the only place that names `ClaudeCode`.
  - `build_registry` calls it with the real Keychain, and keeps its signature for its held
    callers.
  - `statusline::engine` calls it with `NoKeychain` (`statusline.rs:242` today builds its own
    registry). The statusline's walls stay its own.
- **Completions** derive from that function (`cli.rs:594`, `COMPLETED_PROVIDERS`).
  `PossibleValue::new(String)` works, because clap has the `string` feature.
- **The test** at `app.rs:2673` becomes "the statusline's registry ids equal `build_registry`'s".
- **The statusline's sighting home** (`statusline.rs:193-199`, `:283-299`) needs no new method.
  Outside a run shell it keeps today's rule (token report §6.3):
  - Read the effective value of the provider's home variable: `session_dir_var()` names it, and
    its value is in `Env.homes` (§5.1).
  - Unset or empty: `Default`.
  - Otherwise compare it with `p.share_policy(&plain).source`. `plain` is `env` with that name
    removed from `homes` and from `vars`, standing in for today's `plain.claude_config_dir =
    None`. For CC that source is exactly `config_home` (`cc/provider.rs:646`). The same
    directory is `Default`; any other is `Elsewhere`.
  - The existing cases (`statusline.rs:614-622`: empty, the default, a link to it, elsewhere)
    must pass unchanged. One more pins a non-default directory as `Elsewhere`.
- **The statusline's setup hint and settings snippet** (`statusline.rs:184-199`) come from
  `statusline_setup()`.
- **The test probes.** `statusline::engine` returns `(Engine, ProviderId, Arc<LazyHttp>,
  Arc<NoKeychain>)`, and the last two exist only so a test can prove nothing reached them
  (CC-O-22). Return them through a `#[cfg(test)]` accessor instead.
- **`shell_init.rs`** builds its wrapper function from each registered provider's id and
  `launch_command()`. Its output for Claude Code stays byte-identical.

### 5.6 The cswap importer

**Today.** `engine/transfer.rs:299-367` (`read_cswap_v1`) builds Claude Code's identity JSON
(`accountUuid`, `organizationUuid`, `oauthAccount`, and the kind `"oauth"`).

**Change.**
- Move it, with its tests, to `tagteam-cc/src/cswap.rs`, reached through `foreign_export()`.
- `ImportRecord` moves to `tagteam-provider`, and the engine re-exports it.
- `decode` takes `&ProviderRegistry`, and `Decoded.cswap` becomes
  `foreign: Option<&'static str>`.

The user-visible import behaviour does not change.

### 5.7 `validate_profile` (rider)

`validate_profile` (`provider.rs:813`) takes seven parameters, including two adjacent `&Path`s
(`cwd`, `program`) that can be swapped without a compile error. It carries the provider crate's
only `too_many_arguments` allow. Replace them with
`ValidateRequest { env, spelling, cwd, program, expect }`. Implementers:
`tagteam-cc/src/provider.rs:789` and `tagteam-fake/src/provider.rs:982`. Callers: `bootstrap.rs:435`
and `launch.rs:473`.

### 5.8 Unsafe code and poisoned mutexes (ARCH-12 part, CC-O-21)

- **cc's `getpwuid_r`.** `passwd_name` (`cc/naming.rs:51-82`) moves into a new `tagteam-provider`
  module (not `process.rs`), as a safe `user_name()`. `tagteam-cc` then declares
  `#![forbid(unsafe_code)]`.
- **The daemon** gets `#![deny(unsafe_code)]` in `lib.rs`, plus one `#![allow(unsafe_code)]` line
  at the top of `server.rs`. That is a one-line hunk; moving `server.rs`'s calls into the provider
  crate waits until hardening has rewritten that file.
- **The comment at `term.rs:1-3`** states the unsafe policy, and is corrected to match.
- **A `lock_unpoisoned` helper** goes into a new small `tagteam-provider` module. It is used by:
  - `mkdir_lock.rs:320,329,336,383,403`, the heartbeat thread, which today dies on poison and
    leaves a held lock looking stale to other processes;
  - `cc/live.rs`'s eight `seen.lock().unwrap()` calls;
  - optional riders: `cc/locks.rs`, `provider.rs`, `atomic.rs`, `transfer_cmd.rs` and
    `logging.rs`.

### 5.9 The neutrality test

Add `tagteam-fake/tests/neutrality.rs`. For every `src/**/*.rs` file it reads the production
half, and skips:
- everything after the first `#[cfg(test)]` that is followed by `mod`;
- whole files declared as `#[cfg(test)] mod <name>;` from their parent, such as the engine's,
  daemon's and TUI's `testutil.rs`;
- lines whose trimmed text starts with `//`.

It asserts the following:
- **The neutral crates** (`tagteam-provider`, `tagteam-core`, `tagteam-engine`,
  `tagteam-daemon`, `tagteam-tui`) contain no `claude` or `anthropic`, matched case-insensitively.
  R5 adds Claude Code's field names: `procStart`, `oauthAccount`, `accountUuid`, `claudeAiOauth`
  and `daemon-worker`.
- **No neutral crate** lists `tagteam-cc` under `[dependencies]`.
- **The binary** (`crates/tagteam/src`) names `tagteam_cc` or `ClaudeCode` only in the
  composition-root module (`app.rs` today, `app/context.rs` after R3).

The allowlist after R1:
- the TUI's two held strings (§5.3), each pinned by file and text, not by line number;
- for the binary: the composition root, and `logging.rs:27-28` if its filter directives name a cc
  target.

The test must fail when a leak is planted on purpose; the builder checks that once.

## 6. R2 — Engine structure

CC-E-5, CC-E-6, CC-E-9, CC-E-11, CC-E-24, the ARCH-6 split (not the relocation), and ARCH-8 for
`switch.rs`. PERF-R-1 also lives in `switch/`, but it needs hardening's vault form, so it is in R4
(§8.11).

### 6.1 `switch.rs` becomes `switch/`

**Today.** `switch.rs` is 2,462 lines, all production, with no unit-test module. About 20
integration-test files exercise it through `Engine::switch`/`SwitchRequest`, plus the hook points
`"freshen-before-lock"`, `"planned"`, `"identity-reverify"`, `"after-journal"`,
`"after-credential"` and `"after-identity"`.

**Step 1: a pure move** into these files:

| File | Contents | ~Lines |
|---|---|---|
| `mod.rs` | request and outcome types, consts and messages, `switch`, `switch_planned`, `without_store`, `replan_polls`, `reverify_identity`, re-exports | 450 |
| `support.rs` | `Axis`, `Held`, `OracleHint`, `refuse_*`, `attributed`, `unless_forced`, `same_generation`, `vault_holds_login`, `has_login`, `matches_vault`, `live_row` | 250 |
| `select.rs` | `Ask`, `Planned`, `Rotation`, `Ranked`, `Rated`, and the picks from `candidate_order` to `auto_refusal` | 620 |
| `plan.rs` | `Plan`, `plan` | 150 |
| `freshen.rs` | `Freshened`, `AutoFreshened`, `freshen_plan`, `due`, `freshen`, `freshen_auto`, `prepare_usage` | 450 |
| `rollback.rs` | `Rollback` | 125 |
| `transact.rs` | `Locked`, `Rederived`, `rederive`, `read_target`, `settle_profile`, `transact`, `settle_outgoing`, `displace_live`, `hold_vault`, `save_unheld`, `apply` | 600 |

**Re-exports.** Other engine files import these through `crate::switch::`: `Axis`, `Held`,
`OracleHint`, `answer_for`, `refuse_unreadable`, `refuse_unsafe_live_reads`, `needs_relogin`,
`cannot_refresh`, `works_until_expiry`, `AutoFreshened`, `AutoPerform`, `SwitchRequest`,
`SwitchTarget` and `SwitchReason`. `recover.rs` and `active.rs` also call `save_unheld` and
`hold_vault`. Re-export all of them.

**Step 2: a `Settle` context.** The bundle `(p, provider, held, force, live_identity, own_secret,
warnings)` is threaded through `transact`, `recover.rs::finish_forward` and `active.rs`. It
becomes `pub(crate) struct Settle` in `support.rs`.
- `save_unheld` and `displace_live` become methods on `Settle`.
- The `before_fallback` closure (`switch.rs:2152-2168`) becomes `|b| cx.save_unheld(self, b)`.

**Step 3: `transact` as named steps.** Today it is one 231-line body (`:1979-2209`). It becomes:
1. `preflight`: the live read, `doomed`, and the refusals.
2. `settle_pre`: rescues and the profile.
3. `settle_branch`: the direct branch or `settle_outgoing`, returning the target secret.
4. `hold_doomed`.
5. `journal_row`.
6. `apply`, with `Rollback`.
7. `outcome`.

Its own arguments shrink to the plan, the locked state, and one `LockedSwitch { store,
account_locks, live }`. `apply` takes `ApplyStep { target, login, outgoing }`.

### 6.2 `too_many_arguments` (CC-E-6)

Ten of the engine's eleven allows go:

| Function | Replaced by |
|---|---|
| `switch.rs:1978` `transact` | `LockedSwitch` |
| `switch.rs:2214` `settle_outgoing` | `Settle` |
| `switch.rs:2372` `save_unheld` | `Settle` |
| `switch.rs:2401` `apply` | `ApplyStep` |
| `recover.rs:239` `finish_forward` | `Settle` plus `Recovery { row, accounts, locks, live, hints }` |
| `recover.rs:362` `capture_rotated_outgoing` | `Settle` plus `Recovery` |
| `bootstrap.rs:283` `bootstrap_profile` | `ProfileCtx { row, profile, cwd, program, guard, lock }` |
| `launch.rs:407` `prepare_quiescent` | `ProfileCtx` |
| `lifecycle.rs:659` `prepare` | `AddRequest { identity, claimed_uuid, position, alias, yes }` |
| `lifecycle.rs:719` `commit_login` | `LoginWrite { identity, kind, secret, position, alias, source }` |

The eleventh, `store/tokens.rs:547`, belongs to hardening and stays.

### 6.3 `doctor.rs` becomes `doctor/` (CC-E-9, ARCH-6 split)

**Today.** `doctor.rs` has 3,250 production lines. One `Run` struct carries 50 methods, and it
imports from 13 engine modules.

**Change.** `Run` stays in `mod.rs`. Child modules hold `impl Run` blocks, and the methods the
parent calls become `pub(super)`. The unit tests move with their helpers.

| File | Contents | ~Lines |
|---|---|---|
| `mod.rs` | options, report, `Engine::doctor`, `Run`, `push`/`store_ref`/`secrets`/`scope`/`finish`, the `Secrets` helpers, `skipped`, `and_list`, `command` | 720 |
| `store.rs` | `store`, `settled_quick_check`, `is_damage`, `out_of_reach`, `restore*` | 260 |
| `files.rs` | `modes`, `temp_files`, `settings`, `log`, and the file helpers | 500 |
| `daemon.rs` | `daemon`, `daemon_running`, `held_by_daemon`, `daemon_service`, `daemon_socket`, `daemon_notify`, the record and service-state checks | 450 |
| `accounts.rs` | `vault_orphans`, `accounts`, `replacement`, `usage`, `auto`, `models`, `quarantine_words` | 560 |
| `recovery.rs` | `rescues`, `displaced`, `journal`, `dead_switch` | 270 |
| `profiles.rs` | `stray_profiles`, `profile_dirs`, `profile`, `spelling`, `splits`, `reservations`, `unknown_entries`, `held_reservation` | 540 |
| `sessions.rs` | `sessions`, `ownership`, `ownership_walk`, `orphan_ownership`, `daemon_check` | 250 |
| `online.rs` | `online`, `keychain_note` | 100 |

After the move:
- Check IDs become associated consts. `"accounts.vault"` alone is written four times today.
- Builders replace repeated fragments: `warn_restore` replaces the nine `.fix(restore(…))`, and
  `store_skipped` the three "store.skipped" texts.
- `core/rank.rs::span`, a duration formatter, moves to `core/fmt.rs`.

The report's order, IDs and texts do not change. `engine/tests/doctor.rs` and
`tagteam/tests/doctor_cli.rs` are the check.

### 6.4 The guard API (CC-E-11)

**Today.** `engine.rs:340-559` holds 14 guard functions:
- `guard_or_refuse`, `_as`, `_within`, `_for`, `_each` and `_each_within`;
- `settle_or_refuse` and `_as`;
- `mutation_guard`, `metadata_guard` and `purge_guard`;
- `guard_recovering`, `guard_recovering_from` and `acquire_guard`.

Each one hard-codes a different combination of parameters, and six of them hard-code the source
`"cli"`.

**Change.** One request type and one entry point:

```rust
pub(crate) struct GuardRequest<'a> {
    providers: &'a [ProviderId],
    timeout: Duration,
    recovery: Recovery,          // Oracle | FingerprintsOnly | DecodableOnly
    refuse_interrupted: bool,
    source: &'static str,        // until R4's §8.7 lands
}
impl Engine { pub(crate) fn guard(&self, r: &GuardRequest) -> Result<(MutationGuard, Blocked), EngineError>; }
```

- `source` defaults to `"cli"`. §8.7 removes the field, and the guard then reads the engine's own
  source.
- `mutation_guard` (pub), `metadata_guard`, `purge_guard` and `settle_or_refuse` stay as thin
  named wrappers, and `acquire_guard` stays as the primitive.
- About 20 call sites in 9 files change.

### 6.5 Placement (CC-E-24)

- **A new `engine/fsutil.rs`** holds `trace_path`, `absent`, and the `(dev, ino)` file id. That
  id is renamed `FileId`, because `lifecycle::identity` collides with `tagteam_provider::Identity`.
  These move out of `lifecycle.rs:197-256`, which `purge.rs` imports them from today.
- **`seed_launches`** (`engine.rs:~236`, a directory scan with marker parsing) moves to
  `session.rs`.

## 7. R3 — CLI and the daemon protocol's typed values

CC-O-1, -2, -3, -11, -12 (part), -15 (part), -19 (part), the ARCH-12 `local_time` part, the
daemon-side half of ARCH-3/CC-O-7, and CC-O-23a. Nothing here edits a held file (§3.4) except to
keep it compiling.

### 7.1 `app.rs` becomes `app/` (CC-O-1)

`app.rs` has 2,506 production lines.

| File | Contents | ~Lines |
|---|---|---|
| `mod.rs` | `App`, `Ended`, `run`, `run_command`, `command_name`, `App` helpers | 440 |
| `early.rs` | statusline, shell-init, completions, the tui refusal | 110 |
| `context.rs` | env consts, `Context`, `Overrides`, `Io`, `registry`/`build_registry`, `command_settings`, `build_engine`, `ColorEnv`, `locate_run_shell`, `engine_for` | 300 |
| `failure.rs` | `KIND_*`, `Failure` and its `From` impls, `fail`, `fail_purge`, `error_json`, `Failure::report` | 130 |
| `dispatch.rs` | `dispatch` | 70 |
| `commands/accounts.rs` | list, status, remove, alias, move, add, switch, resolve, confirming | 400 |
| `commands/launch.rs` | `run_agent`, `run_plain` | 90 |
| `commands/transfer.rs` | export, import, recipients, passphrase, token | 215 |
| `commands/daemon.rs` | auto, daemon*, `auto_flags`, `auto_failure`, `install_failure` | 290 |
| `commands/tokens.rs` | history, tokens | 115 |
| `commands/mappings.rs` | map, unmap, displaced, path helpers | 150 |
| `commands/config.rs` | config, purge, doctor | 135 |

- `dispatch`'s inline `List`, `Status`, `Remove`, `Alias`, `Move`, `Doctor` and `Auto` bodies
  become methods.
- `tagteam/tests/app.rs` uses only `run`, `Context` and `Io`, which stay re-exported. So do
  `app::UNLOCK_QUESTION`, `build_registry`, `build_engine` and anything else the held
  `tui_host.rs`/`tui_cmd.rs` import.
- **An exhaustive dispatch.** Five arms are `unreachable!` today (`app.rs:1422-1425`, `:1456`),
  because `run_command` answers those commands earlier, at three different stages:
  - `Statusline` and `Tui`, before the registry;
  - `ShellInit` and `Completions`, after the run-shell check;
  - `Run`, after `App` exists.

  Split the clap command enum with `#[command(flatten)]` into groups, so each stage matches
  exhaustively. **Spike this first.** If help order, completions output (`tests/completions.rs`)
  or `tests/tui_args.rs` change, keep the single enum and its `unreachable!` arms, and record why.
- **`alias`** keeps its runtime usage check (C-10).

### 7.2 Bootstrap (CC-O-2)

**Today.** "Registry, then `locate`, then the unreadable-marker refusal, then `build_engine`" is
written at `app.rs:778-826`, `tui_cmd.rs:40-61` and `:127-137`, and `tui_host.rs:48-53`.

**Change.** Two functions in `app/context.rs`:

```rust
pub(crate) fn locate_run_shell(ctx: &Context) -> Result<Located, EngineError>; // registry, detect_run_shell, the RunShellUnreadable error
pub(crate) fn engine_for(ctx: Context, provider: Option<&ProviderId>) -> Result<(Engine, Vec<String>), EngineError>;
```

- `run_command` uses them.
- `locate` stays as a thin wrapper while the held `tui_cmd.rs` and `tui_host.rs` call it. The TUI
  redesign moves them onto `engine_for` (§10).
- The statusline's walled engine stays separate, but uses `app::registry` (§5.5).

### 7.3 Reporting (CC-O-3, CC-O-15 part)

- **`Failure::report(self, out: &mut Output, signals: bool) -> Ended`** is shared by
  `run_command` and `run_statusline`.
  - The statusline passes `signals: false`, which keeps today's behaviour (a signal there exits
    1) and now documents it.
  - The pre-`Io` sites in `lib.rs:93-103`, `:130-134` and `:162-169` use a small variant of the
    same helper.
  - `tui_cmd.rs:30-33` is held and keeps its own copy.
- **`Io::warn`, `Io::note` and `Io::fail`** replace the 12 `writeln!(…, "warning: {w}")` sites,
  the 2 `note:` sites and the hand-written error printing.
- **`App::refuse_in_run_shell()`** calls `Engine::refuse_inside_run_shell` (made `pub`). It
  replaces six copies (`app.rs:1185,1603,1738,1774,1814,1868`); `auto`'s `--dry-run` exemption
  stays.
- **A `SCHEMA_VERSION` constant** replaces 39 `"schemaVersion": 1` literals across 10 files.
- **`main_with_args`** (`lib.rs:110`, 117 lines) splits into parsing with its JSON error, the
  process-boundary setup, logging and the root refusal, and signals.

### 7.4 Small items

- **CC-O-11.** The completions patching (`cli.rs:676-884`) moves to `completions.rs`, with its two
  duplicated collectors extracted. `app.rs:809`'s discarded `write_all` result is handled like
  any other command's output write, so a failed write no longer exits 0.
- **CC-O-12 (part).**
  - `ColorEnv { no_color_flag, no_color_env, force_color_env, stdout_terminal }`, with
    `fn enabled(&self, setting)`, replaces the four booleans that are copied around
    (`app.rs:647-701`).
  - `color_enabled` stays as a wrapper for the held `tui_cmd.rs:69`.
- **CC-O-19 (part).** Core's `Strategy` (Best, ConsumeFirst) is renamed `AutoStrategy`, and the
  CLI's hand maps (`app.rs:489-494`, `:550-553`) follow. The CLI's clap enums stay, so clap stays
  out of the engine. The TUI's `host::Strategy` is held (§10).
- **ARCH-12 (part).** `tagteam/src/auto.rs:316` `local_time` (via `localtime_r`) is replaced by
  `jiff`, as `tui_cmd.rs:84` already does. `local_time_is_a_time_of_day` is the check.

### 7.5 The daemon protocol's typed values (ARCH-3, CC-O-7: daemon side)

**Today.**
- **The server is typed:** `Mode` and `WaitingFor` (`daemon/events.rs:15,35`), `OutcomeKind`
  (`daemon/status.rs:67`) and `NoSwitchReason` (`core/autoswitch.rs:160`).
- **The client structs are strings:** `StatusProvider { mode, waiting_for }`,
  `StatusOutcome.kind` and `Event::Mode` (`protocol.rs:481-561`), and `Event::Auto` keeps the
  `raw` JSON.
- **Consumers re-read the strings:**
  - the TUI decides behaviour on literals, such as whether the daemon collects for it
    (`tagteam-tui/src/daemon/mod.rs:374`, `:461-465`);
  - it reads raw JSON keys (`daemon/words.rs:114-162`);
  - the CLI compares a `String` reason with `NoSwitchReason::X.as_str()` (`auto.rs:237,243`).

  A server-side rename compiles and silently changes behaviour; the worst case spends the request
  budget (core R3).

**Change (additive, C-11; no wire change):**
- **Parsing.** `Mode`, `WaitingFor`, `OutcomeKind` and `NoSwitchReason` gain `parse` (or
  `FromStr`) and `ALL`. The server enums stay without an unknown variant.
- **`Wire<T>`.** `protocol.rs` gains `pub enum Wire<T> { Known(T), Other(String) }`, built from a
  string. `Other` keeps an unknown spelling so a client can print it, which is why
  `#[serde(other)]` is not used. D5.6 forward compatibility holds.
- **Typed accessors beside the existing fields:**
  - `StatusProvider::mode() -> Wire<Mode>`;
  - `StatusProvider::waiting_for() -> Option<Wire<WaitingFor>>`;
  - `StatusOutcome::kind() -> Wire<OutcomeKind>`;
  - the same for `Event::Mode`.

  The `String` fields stay, so held code compiles unchanged.
- **Predicates.** `StatusProvider::collects_for_itself()` and `::waits_for_upgrade()` live in
  `protocol.rs`, beside the producer. They encode the rules the TUI holds today
  (`tagteam-tui/src/daemon/mod.rs:374`, `:459-466`). For an unknown value they keep today's
  default: not collected elsewhere, and the index kept.
- **Payload parsers.** `Event::Auto` gains parsers for the `poll`, `switch` and `no-switch`
  payloads, returning small typed structs from `raw`. That includes the binding-window rule
  (D6.1) that the TUI's `words.rs` `binding` re-derives today.
- **`event_json`'s hand-built envelope** (`protocol.rs:117-131`) uses `envelope()` (CC-O-23a).
- **The CLI** compares through `NoSwitchReason::parse` (C-8).

**Tests:**
- `Wire<T>` round-trips, with an unknown spelling kept;
- each predicate, including its default for an unknown value;
- each payload parser, against `event_json`'s own output.

## 8. R4 — Engine contracts and PERF-R-1

ARCH-4 (narrow), ARCH-5, ARCH-10, CC-E-1, -2, -3, -4, -7, -10, -13, -16, -18, -20, the scoped
parts of CC-E-8, -12, -19 and -21, and PERF-R-1. Starts after hardening has merged (§3.2).

### 8.1 The switchable-account rule (ARCH-5) and the daemon's raw-row reads (ARCH-4)

**Today: five copies.**

| Site | Rule |
|---|---|
| `daemon/auto.rs:122` | switchable, and the provider is registered (deliberate) |
| `daemon/provider_job.rs:278` | switchable, quarantine allowed |
| `engine/auto.rs:772` | switchable, with quarantine as a separate flag |
| `switch.rs:356-358` (`is_candidate`) | switchable and not quarantined |
| `provider_job.rs:638` | the same as `is_candidate` |

**What a collect round actually picks.** `collect_scheduled` (`collect.rs:265-282`) works in two
phases:
- **Phase 1** collects the live account (`Role::Active`), gated by `Eligibility::Scheduled` on its
  own `next_poll_at` (`store/usage.rs:130`). It skips managed-key kinds and quarantined rows.
- **Phase 2** (`scheduled_candidates`, `collect.rs:300-330`) picks the other switch candidates.
  It excludes the live account, managed-key kinds and anything `switch_candidate` rejects, which
  includes session-owned accounts.

**The divergence.** `provider_job.rs:633-646` (`earliest_next_poll`) claims to count "the
accounts a round can collect". In fact it counts any non-quarantined switchable row. So a
session-owned account, or a managed-key one, can set the daemon's next wake although no round
will collect it, and the daemon sits at its 60 s floor (`provider_job.rs:~103-111`).

**Change.**
- `impl AccountRow { pub fn is_switchable(&self) -> bool; pub fn is_poll_candidate(&self) -> bool }`
  in `store/mod.rs`.
- Engine queries for the daemon:
  - `Engine::switchable_count(&ProviderId)`;
  - `Engine::switchable_providers(only)`;
  - `Engine::earliest_poll_at(&ProviderId)`.
- `earliest_poll_at` returns the earliest `next_poll_at` over the accounts a round could ever
  collect, whether or not they are due now:
  - phase 1's live account, unless it is a managed-key kind or quarantined;
  - phase 2's structural candidates: not live, not a managed-key kind, and accepted by
    `switch_candidate`.

  It must not apply the current due test (`Eligibility::Scheduled`, `backoff_holds`) or
  `scheduled_pick`'s selection. Those decide what to collect *now*; a deadline in the future is
  exactly what the daemon schedules on.
  - Extract the structural test (which rows are candidates at all) from `collect_scheduled` and
    `scheduled_candidates` into one function. Collection then filters by due-ness on top of it,
    and `earliest_poll_at` takes the minimum `next_poll_at` over it. The two can then not drift
    again.
- The daemon's `auto.rs:117-128` and `provider_job.rs:273-280` and `:633-646` call these.
  `daemon/auto.rs` keeps its registry clause.
- `notify.rs:442-451` also takes leases directly, but stays as it is (hardening's file).

### 8.2 Lock tokens on store mutators (ARCH-4)

Extend the capability-token pattern that `Vault::store(&AccountLock, …)` and
`CredLocks::new(&MutationGuard, …)` already use:
- `insert_journal(&MutationGuard, …)`;
- `begin_replacement(&AccountLock, …)`;
- `finish_replacement(&AccountLock, …)`.

About 21 engine test files change mechanically, most of them through
`tests/common/mod.rs:191,689,720,734,739`. Per C-1, the `Store` and its other mutators stay `pub`.

### 8.3 The refresh-gate outcome table (CC-E-4)

**Today: three copies of one table.**
- `switch.rs:1348-1480` `freshen` (manual);
- `switch.rs:1487-1570` `freshen_auto` (auto);
- `launch.rs:254-330` `freshen_for_launch` (launch).

The intentional differences:
- launch joins session-owned accounts;
- auto skips or quarantines where manual refuses;
- manual replans only for a chosen target;
- auto carries no user-facing warnings.

The drift: for `Transient { kind: "rescue-unreadable" }`, manual names the damaged files and
launch prints fixed text. Launch also rebuilds `ProfileConflict` inline (`launch.rs:323,421,629`)
although `profile_conflict()` exists.

**Change.** A pure classifier in `switch/freshen.rs`, not in `refresh.rs`:

```rust
pub(crate) enum GateMode { Manual { chosen: bool }, Auto, Launch }
pub(crate) enum GateVerdict { Proceed, ProceedWarn(String), Replan, Refuse(EngineError), Skip, Quarantined, Failed(FailureKind) }
pub(crate) fn classify_gate(&self, out: GateOutcome, p: &dyn Provider, row: &AccountRow, mode: GateMode) -> Result<GateVerdict, EngineError>;
```

- The three callers keep their pre-gate work (locking, settling, the due check), and map the
  verdict to their own result types.
- Launch's `Freshened` struct is renamed `LaunchFresh`.
- Launch uses `profile_conflict()`.
- The rescue-unreadable text becomes the file-naming one for launch too (C-2).
- The existing manual-versus-launch difference for a quarantined row (`launch.rs:~264`
  short-circuits) is preserved, as are all the intentional differences above.
- Tests: `tests/freshen.rs`, `launch.rs` and `auto_tick.rs`, plus one new launch test for the
  rescue text.

`collect.rs:895-914` (`gate`) maps every non-success outcome to `refresh-failed`. That is a
different policy, and it is not folded in.

### 8.4 The never-delete-the-live-login guard (CC-E-10, CC-E-20)

**Today: two copies of the walk.**
- `lifecycle.rs:470-531` (`refuse_live_files_at`), called from eight sites.
- `purge.rs:912-1022` (`refuse_live_in_envelope`), 112 lines with 6 levels of nesting.

Both resolve the providers' `identity_surface` files, walk their ancestors, compare
`(dev, ino)`, and refuse.

The intentional differences:
- lifecycle's doomed set is one entry;
- purge's is the data directories, minus spared profiles and the directories on the way to them;
- purge also records the followed `metadata` of each ancestor.

The drift:
- **Message wording:** "would break it" vs "would delete it", and "names it" vs "names them".

**Change.** A new `engine/live_guard.rs`:

```rust
pub(crate) fn touched_chain(file: &Path) -> Result<Vec<FileId>, LiveGuardError>; // the trace_path and ancestor ids, both unfollowed and followed
pub(crate) enum LiveGuardError { Unresolved(Scope), Inside(Scope) }               // Scope = ProfileEntry | Purge
```

- Both callers keep their own idea of what is doomed, and share the walk.
- Each scope keeps its exact current text.
- The walk uses the superset of ids, unfollowed and followed, in both scopes.
- The integration tests `tests/purge.rs`, `displaced.rs` and `replacement_unreadable.rs` are the
  safety net, and must pass unchanged.

**CC-E-20.** `deletion_targets` (`purge.rs:884,888`) swallows listing errors with
`unwrap_or_default()` inside the pre-delete check. It returns `Result` and propagates them, so a
rescue directory that becomes unlistable between plan and execution refuses instead of narrowing
the check.

### 8.5 Error kinds (CC-E-3, ARCH-10)

Per D-3, kind values never change, but a refusal may move to a kind that describes it.

**New `EngineError` variants:**

| Variant | `kind()` | Replaces |
|---|---|---|
| `DeletionRefused { path: Option<PathBuf>, why: DeletionRefusal }` (`LiveLoginKeychain`, `LiveLoginUnresolved`, `LiveLoginInside`, `StoredAccountKeychain`, `StoredAccountUnresolved`) | `deletion-refused` | `EngineError::Io(io::Error::other(…))` at `lifecycle.rs:450,476,528` and `purge.rs:790,808,919,924,1116` |
| `ProfileMarkerMissing { path }` | `profile-marker-missing` | `InvalidInput` at `launch.rs:238` and `bootstrap.rs:314` |

**Message text** stays the same, including the path prefix.

**The rewrap sites.** `purge.rs:291,1034,1317,1323` rewrap an existing `EngineError` as `Io` with
a path prefix. They use a path-carrying wrapper that reports the inner error's kind, so each now
reports what it is instead of `io` (§11).

**The remaining `InvalidInput` state refusals** are at `bootstrap.rs:117,165,343`,
`switch.rs:1264,1335,1943`, `active.rs:438,444` and `profiles.rs:95,323,329,557`. The planner
classifies each one:
- if an existing kind describes it exactly (for example `needs-relogin`), it moves there;
- otherwise it stays `invalid-input`.

The plan lists each decision.

**`kind()` and `signal()`** (ARCH-10.1) share one `fn lock_error(&self) -> Option<&LockError>` on
each carrier. That fixes an existing inconsistency: `Lock(Compromised | Io)` reports `lock`, but
`Provider(Lock(Compromised | Io))` reports `provider`; both report `lock` afterwards.
`ProviderError::RestoreFailed` keeps its own handling.

**Tests.** `kind_is_pinned_for_every_variant` (`error.rs:535`) is extended. New tests pin
`deletion-refused` through `purge --json` and `remove`.

**`VaultError::Write` and `Delete`** (ARCH-10.2, `vault.rs:15,17`) keep their `KeychainError` as
`source` instead of flattening it to a `String`. The message is unchanged. This edit goes on top
of hardening's rewrite of `Vault::store` (H5.3).

### 8.6 Failure kinds (CC-E-1)

There are two namespaces.

1. **Usage failure tokens.**
   - **Where they are stored:** `usage_state.last_error`, shown as `usageError`.
   - **Writers:** about 24 sites in `collect.rs`.
   - **Readers:** `views.rs:175-178` and `words.rs:125`.
   - **The tokens:** `no-access-token`, `http-401`, `refresh-failed`, `keychain-unavailable`,
     `profile-drifted`, `live-replaced`, `foreign-credential`, `vault-absent`, `token-expired`,
     `over-budget`, plus the provider's `TransientKind::token()` values.
   - **`over-budget`** is spelled five ways today (`store/usage.rs:51`, `collect.rs:1105`,
     `auto.rs:857`, `provider_job.rs:613`, `words.rs:125`).

   **Change:** `enum FailureKind` with `as_str`/`parse`. Its variants are the ten tokens,
   `Provider(TransientKind)`, and `Other(String)` for values read back from the store.
   - The string form in the store is unchanged.
   - `Collected::Failed`'s `"error"` sentinel (`collect.rs:111`) leaves the token space and
     becomes `Collected::Errored`. The wire is unchanged: wherever that outcome is serialised
     today, such as the daemon's `fetchErrors`, it is still sent as `"error"`.
   - The held `tagteam-tui/src/source.rs:166` (`kind != "error"`) gets only the minimal edit that
     keeps it compiling (§3.4).
   - Unmapped tokens still read as `UsageStatus::Unavailable`, which `views.rs:1003-1015` pins.
2. **Gate transient kinds**, in `GateOutcome::Transient { kind }`. Their writers are in
   `refresh.rs`, `active.rs:409` and `switch.rs:1518,1531`, and their readers match
   `"rescue-unreadable"`. They become a small `enum GateFailure`.

### 8.7 Event source and kind (CC-E-2)

**Today.**
- `source` travels as `&'static str`, and `"cli"` is a literal at 12 production sites.
- The gate's quarantine and unquarantine calls hard-code `"cli"` (`refresh.rs:352,405`,
  `quarantine.rs:90`). So a quarantine change raised by `auto`, or by the daemon's collector, is
  recorded with source `cli`.
- Guards on daemon-reachable paths record `"cli"` too: `collect.rs:938`'s guard, and the
  replacement unquarantine at `store/mod.rs:1185`. So does a recovered switch under them.
- That breaks the promise in `engine/auto.rs:417`'s `with_source` doc. Only the `events.source`
  column and the INFO log line show it.

**Change.**
- `enum EventSource { Cli, Auto, Daemon }` and `enum EventKind` (switch, switch-recovered,
  quarantine, unquarantine, add, remove, …).
- `EngineConfig` gains `source: EventSource`. The CLI sets it per command: `auto` for
  `tagteam auto`, `daemon` for the daemon, `cli` otherwise, the TUI's lanes included.
- The gate, the guards (removing §6.4's `source` field), recovery and quarantine read
  `self.source()`, and `AutoEngine::with_source` goes.
- `EventRow` and the store's event functions take the enums; the stored strings are unchanged.

### 8.8 The settings registry (CC-E-7)

**Today.**
- 17 keys in `KEYS` (`settings.rs:187-290`) are dispatched by two 17-arm string matches:
  `value` (`:868-893`) and `apply` (`:898-932`). Each ends in `unreachable!`.
- `ui.color` silently falls back to `Auto`, and `strategy` to `Best`.
- `STRATEGIES`/`COLORS` duplicate the enums' `as_str`.
- The ranges are declared twice.

**Change.**
- `enum KeyId`. `Key` carries its `KeyId`, and `value`/`apply` match on it exhaustively, so a
  missing arm is a compile error.
- `Choice` values derive from `AutoStrategy::ALL` and `ColorMode::ALL`. `ColorMode::parse` is
  added.
- Ranges are declared once.
- `KEYS` stays a `&[Key]` for completions (`cli.rs:615-617`) and `config`.

### 8.9 Small items

- **CC-E-8 (scoped).**
  - `Engine::now_s()` and `ms_to_s_ceil` replace about 14 inline `div_euclid(1000)`
    conversions, including `collect.rs:677`'s private `now_s`. The daemon's `server.rs:446` and
    the held TUI's `source.rs:112` are skipped.
  - `doctor.rs:1679-1681`'s truncating `/ 1000` becomes `div_euclid`.
- **CC-E-12 (scoped).** One helper parses a stored quarantine reason with an explicit fallback,
  used at `active.rs:286`, `auto.rs:184`, `doctor.rs:1666` and `refresh.rs:402`.
- **CC-E-18 + CC-E-19 (part).** Serde structs replace the hand-written JSON:
  - `ReplacementMeta` covers `store/mod.rs:1082-1093` (write), `1170-1190` (read) and `:364`
    (`replacing_meta_parses`);
  - the journal structs cover `journal_to_json`/`journal_from_json` (`:275-321`), and
    `u32::try_from` replaces the cast at `:306`.

  `set_wal_mode` reuses `BUSY_TIMEOUT` (`:642`).
- **CC-E-13.**
  - `display_email()` on `Identity` and on `AccountRow` replaces 14 inline copies.
  - A shared `read_lock_record(path)` serves `auto.rs:312` and `daemon.rs:70`.
  - `poll::jittered` is shared with `autoswitch.rs:817`.
  - One cache-or-open helper serves `Engine::store`/`existing_store`.
- **CC-E-16.** `auto.rs::perform` (`:578-720`) extracts `try_target(..) -> TargetStep` and stops
  cloning `decision` five times.
- **CC-E-21 (part).** New enums (§8.5 to §8.8) use a small `str_enum!` macro for `as_str`/`parse`.
  Existing hand-written pairs convert only where this branch touches them anyway.

### 8.10 Order inside R4

1. The policy and typing items (§8.1 to §8.9).
2. PERF-R-1 (§8.11), last, because its budget test should measure the finished switch.

### 8.11 PERF-R-1: fewer Keychain calls per switch

**The recount** was made at `683b266`, before hardening. A manual macOS switch makes about 26
`/usr/bin/security` calls, in this scenario:
- an OAuth target and a managed outgoing account;
- the live credential equal to the vault's;
- no profile or rescue;
- no managed-key item.

About 20 of those calls run with the mutation, account and CC credential locks held, from
`rederive` onward. Fourteen are live-credential reads that core §9.1 deliberately keeps fresh
(the `EntryMoved` rule), so they are not cached.

**After hardening.** H4 changes how each call is run, but not how many there are. H5.3 changes
what "equal to the vault" means: after it, the live credential's vault form is what equals the
vault. The planner re-counts on the rebased base, and states the scenario in those terms.

**Changes:**
1. **A per-transaction vault-read memo** in the engine.
   - It is built in `switch_planned` right after `lock_accounts`, and takes `&[AccountLock]` as
     type-level proof that the locks are held. It is dropped with them.
   - `Vault` gains a write version (an `AtomicU64` that `store` and `delete` bump). An entry made
     at an older version is ignored, so every direct `vault.store` caller invalidates it without
     any discipline. Those callers are `rescue.rs`, `provenance.rs`, `settle_outgoing`'s capture,
     and hardening's rewritten `store` path.
   - Other processes cannot write the vault meanwhile, because writes require the account lock
     (`vault.rs:148-152`).
   - It is never a cache on the shared `Vault` or `Engine`, because the daemon shares those
     across threads.
   - It is passed through `Locked`/`Settle` into `read_target`, `settle_outgoing` and
     `hold_vault`. The `hold_vault` callers in `recover.rs` and `active.rs` use an uncached pass.
2. **`rederive`'s read enters the memo.** `rederive` reads the target under the locks; that read
   is recorded in the memo at the current write version.
   - `transact` reads through the memo, so it reuses those bytes only while no vault write has
     happened since.
   - `settle_rescues`, `settle_profile` or the self-switch capture can rewrite the target's
     entry after `rederive`, and that write bumps the version, which forces a fresh read.
   - Handing the bytes over directly would risk activating a spent generation, so it is not done.
3. **A Keychain-call budget test.** An engine test counts Keychain calls for the scenario above,
   through a counting wrapper that does not edit hardening's fakes, and pins the number. The
   count may go down later; going up fails the test, which then has to be changed on purpose.
4. **`clear_managed_key`** (`cc/live.rs:697-706`, `:988-999`) skips its unconditional delete and
   exists probe when the strict read, under the same hold of CC's storage-write lock, already
   found no managed-key item. That saves 2 calls under the locks.
   - **Condition:** the planner verifies that the read and the delete happen under one lock hold,
     and that core §9.1's ledger semantics allow the skip.
   - If they do, core §9.1 gets a one-sentence amendment.
   - If either fails, this item is dropped and the PR records why.

**Expected result:** about 5 fewer calls than the rebased count (26 at `683b266`). Larger gains
would need weakening the §9.1 freshness rule, which is an anti-goal.

**Rejected:** `has_login` via `Keychain::exists`. It is the same `find-generic-password` without
`-w`, so it saves no call. It would also count an empty vault item as "has a login", which in
`rederive` turns a clean `Replan` into an error inside `transact`.

## 9. R5 — Provider boundary, step 2: session records

Waits for hardening (§3.2).

**Today.** `tagteam-provider/src/liveness.rs` parses Claude Code's session-record JSON (`pid`,
`procStart`, `startedAt`, `sessionId`, `kind`, the all-digit legacy form) at `:14-26` and
`:67-94`, and its `daemon.lock` at `:175-198`.
- It also holds `parse_lstart` (`:237-269`, CC's `ps -o lstart` encoding) and the legacy branch of
  `record_is_live` (`:535-539`).
- The engine reads records with that parser at `session.rs:11,245,250,275-278` and
  `tokens/evidence.rs:14-15,128,141,451,729`, and assumes `<pid>.json` at `evidence.rs:431,723`.
- It names CC's kinds in `session.rs:126` (`DAEMON_KINDS = ["bg", "daemon", "daemon-worker"]`)
  and `daemon.lock` in `error.rs:300`.
- `FakeAgent` writes CC-format records (`tests/fake_sessions.rs:79-88` uses CC's `record_json`),
  so the guard cannot see any of this.

**Change.**
- **The neutral DTO:**
  `SessionRecord { pid, start: ProcStart, started_at_ms, session_id, background: bool }`. The
  liveness judgement stays neutral.
- **`ProcStart` must keep every distinction `record_is_live` (`liveness.rs:520-560`) makes
  today**, with the same outcome for each:

  | Variant | Today's input | Today's outcome |
  |---|---|---|
  | `EpochS(i64)` | `lstart` text that `parse_lstart` reads | live if the OS start time is within ±1 s or unknown; otherwise live only if the process mentions the launch command (unknown counts as mentioning) |
  | `BootTicks(u64)` | the all-digit legacy form that fits a `u64` | live if the OS start ticks match; live when the ticks are unknown |
  | `BootTicksIndeterminate` | the all-digit legacy form that overflows `u64` | live, because the failed conversion counts as live today |
  | `Unknown` | absent, or text that is neither | the 120 s `startedAt` heuristic |

  `evidence.rs`'s readers and the supervisor-lock reader keep their current outcomes in the same
  way. A test covers every row of the table, including the overflowing legacy value.
- **Two trait methods:** `fn parse_session_record(&self, bytes: &[u8]) -> Result<SessionRecord,
  String>` and `fn session_record_ext(&self) -> &'static str`. Files are `<dir>/<pid>.<ext>`.
- **cc** parses its JSON, `lstart` and the legacy form, and maps its kinds to `background`, so
  `DAEMON_KINDS` leaves the engine. cc reads the supervisor lock through `supervisor_lock()` plus
  `parse_session_record`. `error.rs:300` stops naming `daemon.lock`.
- **The neutral lister** becomes `read_session_records(dir, ext, parse, clock)`. It is per record,
  not a whole-directory `session_records(profile)` method, because `evidence.rs` needs
  single-record reads by pid and dated reads with a clock hook.
- **FakeAgent's own format:**
  - files `procs/<pid>.rec`;
  - content `{"process":{"id":N,"since":<epoch s>},"session":…,"role":"keeper"|…}`, which takes
    the `EpochS` path;
  - its own supervisor file, `procs/keeper.lock`;
  - a `tagteam_fake::session_record(..)` helper, which `fake_sessions.rs` uses instead of CC's
    `record_json`.
- **The neutrality test** adds Claude Code's field names (§5.9).
- **xtask's compat harness** follows the moved imports.

About 600 of the roughly 1,000 changed lines are code and tests moved from `liveness.rs` into cc.

## 10. Held for the TUI redesign

These items stay out of this branch. The TUI redesign's worktree, which owns the held files
(§3.4), picks them up, or the coordinator routes them.

| Item | What was settled here |
|---|---|
| ARCH-7, CC-O-20 | Michael's D-2: keep argv, and fix the two couplings. **The unlock prompt:** `Prompter::confirm_unlock()`, defaulting to `confirm(UNLOCK_QUESTION, true)`, called at `app.rs:1207` and overridden by `tui_host.rs`'s `Bridge`; this replaces `question == app::UNLOCK_QUESTION` at `tui_host.rs:265`. **The warnings:** the host passes the warnings it has already shown into the run, as a field on `Context`, instead of `replacen` on captured stderr (`tui_host.rs:69-74`) |
| ARCH-3, CC-O-7 (TUI side) | Adopt R3's accessors and predicates (§7.5) in place of the literals at `tagteam-tui/src/daemon/mod.rs:41-44,312,374,461-465` and the raw keys in `daemon/words.rs`. The `String` fields in `protocol.rs` can then be retyped to `Wire<T>` |
| CC-O-8 | The Elm-boundary leaks: the `Sender` stored in `State` (`actions.rs:31-35,59,376,504`); `banner`/`banner_from` written separately (`state.rs:57-60`, `runtime.rs:749-750`); the runtime writing `quitting` and `running`; the reload block copied five times. Suggested shapes: an `AnswerTo` token with `Cmd::Answer`, one `Banner` type, runtime messages, and `State::reload_views` |
| CC-O-9 | `overlay_key`'s take-and-rebuild (`actions.rs:315-498`), and `render_overlay`'s unused `now_ms` |
| CC-O-10 | The four copies of the lane spawn in `runtime.rs:236-375` |
| CC-O-16 (TUI part) | `highlight` and `keep_visible`, duplicated across `screens/accounts.rs` and `screens/tokens/table.rs` |
| CC-O-18 | `tagteam-tui`'s public modules that only the crate uses |
| CC-O-19 (TUI part) | The TUI's `host::Strategy` duplicates the engine's `UsageStrategy` |
| CC-O-23c, f | The TUI's `Result<Status, String>` drops `ClientError`; `daemon/words.rs` re-derives "used = 100 − headroom" |
| CC-O-2, CC-O-3 (TUI sites) | `tui_cmd.rs` and `tui_host.rs` move onto `engine_for` and `Failure::report` (§7.2, §7.3), retiring the `locate` and `color_enabled` wrappers |
| ARCH-1 (TUI wording) | The two strings allowlisted in §5.9 come from the provider (`display_name()`, `launch_command()`), and their allowlist entries are removed |

## 11. User-visible changes

Everything else stays byte-identical (§1.4). Each change gets a `CHANGELOG.md` entry in its PR.

| PR | Change |
|---|---|
| R1 | Clap help for provider-generic behaviour drops "Claude Code" (C-4) |
| R1 | `config`'s description of `default_provider` drops its "such as \"claude-code\"" example (C-9) |
| R3 | `tagteam completions` exits non-zero when it cannot write its output (§7.4) |
| R4 | `--json` `error.type`: the refused deletions above report `deletion-refused` (they reported `io`); a missing profile marker reports `profile-marker-missing` (it reported `invalid-input`); `Provider(Lock(Compromised\|Io))` reports `lock` (it reported `provider`); purge's path-prefixed rewraps report their inner error's kind (they reported `io`); plus any `invalid-input` sites the plan moves to an existing kind (§8.5) |
| R4 | `tagteam run` names the damaged files when a pending rescue cannot be read (C-2) |
| R4 | The daemon no longer wakes at its 60 s floor for accounts collect will skip (§8.1) |
| R4 | Events raised under `auto` or the daemon record their real source in `events.source` and the log. That covers quarantine and unquarantine, recovered switches, and the replacement unquarantine (§8.7) |
| R4 | A switch makes fewer Keychain calls, two fewer under the locks if §8.11 item 4 holds |

## 12. Testing

- **Existing suites are the net.** The pure moves (R2, R3 and part of R1) change no test. Edits
  to tests are limited to:
  - §5.3's list;
  - mechanical signature changes (§6.2, §8.2);
  - held-file compile fixes (§3.4);
  - the new tests below.
- **New tests:**
  - the neutrality test (§5.9), checked once against a planted leak;
  - the protocol's `Wire<T>`, predicates and payload parsers (§7.5);
  - completions' failed write (§7.4);
  - `classify_gate`'s table, one case per row of §8.3, through the three callers;
  - the rescue-unreadable text for `run` (C-2);
  - `deletion-refused`, `profile-marker-missing` and the rewrap kinds through `--json` (§8.5);
  - the `kind()` fix for `Provider(Lock(..))` (ARCH-10);
  - `earliest_poll_at` (§8.1):
    - it includes the live account's planned poll;
    - it includes a candidate whose `next_poll_at` is in the future (for example 120 s ahead
      with a 300 s interval);
    - it ignores session-owned candidates, managed-key kinds and quarantined rows;
    - it uses the same structural candidacy as `collect_scheduled`;
  - `apply_outer_home`'s two rules, unconditional for `homes` and tracked-only for `vars`, across
    a nested run shell (§5.1);
  - every branch of `record_is_live` through the new `ProcStart`, including an all-digit start
    that overflows `u64` (§9);
  - events recorded with `auto` and `daemon` sources (§8.7);
  - the Keychain-call budget (§8.11);
  - FakeAgent's own record format through `tests/fake_sessions.rs` (§9).
- **Linux.** The full suite runs in the Docker repro before each PR is reported done.

## 13. Triage of every finding

| ID | Disposition |
|---|---|
| ARCH-1 | R1 (§5) and R5 (§9); the TUI's wording is held (§10) |
| ARCH-2 | R1 §5.5 |
| ARCH-3, CC-O-7 | Daemon side: R3 §7.5. TUI side: held (§10) |
| ARCH-4 | R4 §8.1–8.2, narrow (C-1). `take_notice_lease` dropped: `notify.rs` belongs to hardening |
| ARCH-5 | R4 §8.1 |
| ARCH-6 | Split: R2 §6.3. Relocation: dropped for now (§1.3) |
| ARCH-7, CC-O-20 | Ruled (D-2) and held (§10) |
| ARCH-8 | `switch.rs`: R2 §6.1. `app.rs`: R3 §7.1. The `store/mod.rs` split is dropped: pure churn while R4 edits that file. `purge.rs`, `settings.rs` and `cc/live.rs` each have one responsibility |
| ARCH-9, CC-O-6 | Dropped: core R7 (second-provider spec). `ValidateRequest` done in R1 §5.7 |
| ARCH-10 | 1 and 2: R4 §8.5. 3 (CLI remedies in engine messages): dropped, fine while every frontend renders through the CLI |
| ARCH-11, CC-O-4, CC-O-5 | Dropped: hardening |
| ARCH-12 | cc and daemon: R1 §5.8. `local_time`: R3 §7.4. Moving `server.rs`'s calls: dropped until hardening's rewrite lands. `own_uid` (geteuid) and `current_uid` (getuid) are different calls, not duplicates |
| CC-E-1 | R4 §8.6 |
| CC-E-2 | R4 §8.7 |
| CC-E-3 | R4 §8.5 (D-3) |
| CC-E-4 | R4 §8.3 |
| CC-E-5, CC-E-6 | R2 §6.1–6.2 |
| CC-E-7 | R4 §8.8 |
| CC-E-8 | Helper: R4 §8.9. Newtypes: dropped (§1.3) |
| CC-E-9 | R2 §6.3 |
| CC-E-10 | R4 §8.4 |
| CC-E-11 | R2 §6.4 |
| CC-E-12 | Fallback helper: R4 §8.9. Typed column and newtypes: dropped (§1.3) |
| CC-E-13 | R4 §8.9, except the two `quoted` functions: dropped, they are unrelated (shell vs systemd quoting) |
| CC-E-14, CC-O-13 | §14: plan-local references rewritten in `src/`; "Decision N" only where code moves |
| CC-E-15 | Folded: `kind`/`source` into §8.6–8.7, `FileId` into §6.5, `Freshened` into §8.3, `Strategy` into §7.4. `Rated` dropped: private |
| CC-E-16 | R4 §8.9 |
| CC-E-17 | `best_pick` and `plan` fold into §6.1. `refresh_with`, `run_gate` and `commit_token_batch` belong to hardening. The rest are dropped: linear and commented |
| CC-E-18 | R4 §8.9 |
| CC-E-19 | `:306` cast and `BUSY_TIMEOUT`: R4 §8.9. The rest dropped: written by the same store from in-range values; `store/tokens.rs` belongs to hardening |
| CC-E-20 | `deletion_targets`: R4 §8.4. `hooks::point` discards: dropped, test-only seams. `active_next_poll_at`: dropped, documented |
| CC-E-21 | `str_enum!`: R4 §8.9. `Eligibility`: folded into §8.1. `Decision.outcome`: dropped, 35 sites including `notify.rs`. `ExitSeverity`, the `Pace` tri-states and doc numbers: dropped, cosmetic |
| CC-E-22 | Dropped: `Work.peak` belongs to hardening; the hook-point names and `bootstrap_quiescent` are test seams behind a feature |
| CC-E-23 | Dropped: the `expect`s cannot fire, and a public `FreshCredential::new` would weaken the provenance proof |
| CC-E-24 | R2 §6.3, §6.5; the `words.rs` helpers go with CC-E-13 |
| CC-O-1 | R3 §7.1. The `alias` `ArgGroup` is dropped (C-10) |
| CC-O-2 | R3 §7.2; the TUI sites are held (§10) |
| CC-O-3 | R3 §7.3; the TUI site is held (§10) |
| CC-O-8, CC-O-9, CC-O-10 | Held (§10) |
| CC-O-11 | R3 §7.4 |
| CC-O-12 | `ColorEnv`: R3 §7.4. `ExportArgs`/`AutoArgs` and the `several` boolean: dropped, cosmetic |
| CC-O-14 | Dropped (§1.3): it touches `refresh.rs` and the identity key |
| CC-O-15 | `main_with_args`: R3 §7.3. The rest dropped: flat matches |
| CC-O-16 | TUI part: held (§10). `Cell`/`Table`: dropped, different types. The CLI's table padders: dropped, cosmetic |
| CC-O-17 | Dropped: not a defect. R5 gives FakeAgent a supervisor file of its own |
| CC-O-18 | Held (§10) |
| CC-O-19 | Core rename: R3 §7.4. TUI part: held (§10) |
| CC-O-21 | R1 §5.8 |
| CC-O-22 | Statusline test probes: R1 §5.5. File splits of `statusline.rs`, `tokens_cmd.rs` and `prompt.rs`: dropped, pure moves |
| CC-O-23 | a: R3 §7.5. b (`DaemonError`): dropped, its one variant leaves room. c, f: held (§10); the other `Result<_, String>` sites are dropped, they are parse results. d: folded into ARCH-12. e: dropped, cosmetic |
| PERF-R-1 | R4 §8.11 |

## 14. Comments that cite plans

About 75 comments in `src/` cite plan-local numbering that does not resolve without knowing which
of 15 plans is meant:
- "Task N", "Plan B", "Controller note", "fix round", "review, item";
- spec line numbers such as "L444".

Each is rewritten as the rule it stood for, or as a spec section reference, in the PR that owns
its file. That includes the stale `bootstrap.rs:465` ("before `launch` exists").

The 333 "Decision N" references stay. They resolve against tracked plans, so they are
ambiguous, not wrong. They are rewritten only where their code moves anyway (§6.1, §6.3, §7.1).

Skipped: hardening's files, the held TUI files, and `tests/`.

## 15. Left to the builder

- **Names:** of new types, fields and helpers, where this spec calls them a shape.
- **Commit granularity** inside each PR, as long as moves stay separate from edits (§3.1).
- **The plan's task order** inside a PR, within the dependencies above.
- **The `invalid-input` classification** in §8.5, recorded in the plan.
- **Whether `EventKind`** covers every event kind written today, or only those the engine
  constructs. Either way, the stored strings stay as they are.
- **Which optional riders in §5.8** to include.

## 16. Amendments to other specs

This spec is the authority for the structure it changes. It adds one-line pointers, and rewrites
no point-in-time facts:
- **Core §4.5** lists the trait additions: `vars`, `words`, `statusline_setup`, `foreign_export`
  (R1), and `parse_session_record`, `session_record_ext` (R5).
- **Core §13.2/§14** state D-3 and the new kinds `deletion-refused` and `profile-marker-missing`.
- **Core §9.1** gets one sentence, only if §8.11 item 4 holds.
