# tagteam review hardening: security and performance — Implementation Plan

**Status:** Implemented — https://github.com/michaelrigart/tagteam/pull/23

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the security and performance findings of the 2026-10-19 codebase review that this branch owns, as the approved spec designs them.

**Architecture:**
- **Release pipeline** (Task 2): no build cache in release builds, attestations on tags, a cargo-deny advisories job.
- **One bounded subprocess runner** (Tasks 3–5) in `tagteam-provider/src/process.rs`, which `security.rs` adapts. It notices exit through `ExitWatch` (`waitid` with `WNOWAIT`) instead of a 10 ms poll, and kills the whole group. Keychain writes are never cancellable; reads are cancellable only inside an opt-in scope around usage collection.
- **Credentials off argv** (Tasks 1, 7, 8): `-i` lines carry escaped `-w` text (proved first on a real keychain), argv is a logged last resort, and the vault and `rescue/` hold the credential without its machine-shared keys (`Provider::vault_form`).
- **Refresh write-ahead** (Task 9): the rescue envelope is written before the vault; one landed rule in every rescue adopter and picker.
- **Idle wakeups**: the daemon's notifier, accept and connection threads block on their descriptors (Tasks 10–11); `tagteam run` waits on `ExitWatch` and ticks at 250 ms (Task 6).
- **Token report** (Tasks 12–13): rows without owned strings, a 12-month bench, and samples queries on the primary key.
- **Small fixes** (Tasks 14–18), **test doubles out of release** (Task 19), and **the amended specs and SECURITY.md** (Task 20).

**Tech Stack:** Rust edition 2024 on 1.88.0; `libc` 0.2 (0.2.189 locked); `rusqlite` 0.40 with bundled SQLite; `serde_json` with `preserve_order`; `signal-hook` 0.3; dist 0.33.0 (from `mise.toml`); cargo-deny through `EmbarkStudios/cargo-deny-action`.

**Spec:** `docs/superpowers/specs/2026-10-19-tagteam-review-hardening-design.md` (Approved, 5c9a114). "H5.3" cites its sections. Read H1, H2 and the sections a task names before starting it. "§7.3" cites the core spec, "D4.6" the daemon spec, "token §6.4" the token report spec.

## Global Constraints

- **Ownership (H1.2):** `crates/tagteam-engine/src/switch.rs`, `launch.rs`, `doctor.rs`, `crates/tagteam/src/app.rs`, `crates/tagteam-daemon/src/auto.rs` and `provider_job.rs` belong to the `review-refactor` branch. This plan touches exactly one line of `app.rs` (Task 14) and at most the arguments of `switch.rs`'s one `vault.store` call (Task 8). Nothing in `crates/tagteam-tui` changes except dev-dependency lines in its `Cargo.toml` (Task 19).
- **No schema version bump** (§6.1's ladder stays at 3). **No new runtime dependency.**
- **`LINE_LIMIT` stays 4032 bytes**, now applied to the escaped line (H5.1).
- **Machine-shared keys:** `mcpOAuth`, `mcpOAuthClientConfig`, `mcpXaaIdp`, `mcpXaaIdpConfig`, `pluginSecrets` (`tagteam-cc/src/shape.rs` `MACHINE_SHARED_KEYS`). `displaced/` stays verbatim (H5.3).
- **Never cancellable:** `upsert` (its verify read included), `delete`, `delete_service`, and every read inside a §14.1 critical span (H4).
- **Cancel slices:** 10 ms while a cancel token is in force; none otherwise (H4).
- **`tagteam run`:** wait tick and launch poller 250 ms; the 2 s sighting write unchanged; only the main thread reaps (H9).
- **Token report budget:** a 12-month report (~1.5M responses) ≤ 1 s release-mode for query and grouping, or stop and report (H10.1).
- **Logging:** never a secret, an email, or a credential's value; account IDs and 12-hex fingerprints only (SECURITY.md).

## Review Focus

1. A credential containing `"`, `\`, JSON escapes such as `\n` or `é` as text, raw non-ASCII, `#`, `$` or tab is stored and read back byte-identical (Tasks 1, 7).
2. A vault item or rescue written before this branch (full bytes, machine-shared keys included) still reads, classifies, switches and settles correctly (Tasks 8, 9).
3. A signal that arrives as `tagteam run`'s child exits is forwarded only while the child is unreaped, never after (Task 6).
4. A daemon shutting down with a subscriber whose queue is full and 16 open connections ends every connection thread and still delivers `stopping` to the others (Task 11).
5. A Keychain read interrupted inside usage collection records nothing and is not a usage failure, while one inside a refresh-gate span is never interrupted (Task 5).

## Rulings

Made while planning, inside the approved spec; each goes under "Rulings" in the MR.

- **R1.** `switch.rs:2043` compares the live entry and the target's vault bytes both in vault form (Task 8 Step 8a), so a pre-branch vault item compares as before: without it, `switch --force` onto the live account displaces its own entry. A third minimal touch of `switch.rs` beyond H1.2's two; the only byte comparison H5.3 breaks (the others compare fingerprints or live bytes with live bytes).
- **R2.** The token report's range read names its index (`INDEXED BY token_usage_by_at`, Task 12): compact rows alone measured 1.38 s against the 1 s budget; with the hint 0.75 s and 194 MiB (from 2.0 s and 451 MiB). The report reads new row types; `UsageRow`, the indexer's write type, keeps its strings.
- **R3.** `rustls` moves from 0.23.45 to 0.23.46 (Task 2): RUSTSEC-2026-0346 to -0349 (published 2026-10-14) affect the locked version, and the new advisory job would fail on them.
- **R4.** `deny.toml` sets `unsound = "all"` (cargo-deny's default is `"workspace"`), matching H3's policy; `ci.yml` gains a weekly schedule, with the `check` and `feature-graph` jobs skipped on it.
- **R5.** The socket client fails closed: a server whose uid cannot be read is no daemon, with a WARN (Task 16).
- **R6.** `render.rs:456-458` (the run shell's unmanaged session login) is sanitized beside `render.rs:431` (Task 15).
- **R7.** The WARN on a readable `~/.claude.json` names no path (core §14.2 keeps home paths out of the log) (Task 17).

## Execution notes

- **Where:** this worktree, `~/Code/tagteam-review-hardening`, branch `review-hardening`. When execution starts, set this plan's `**Status:**` to `In progress` in one commit.
- **Batches.** Every task names its `Depends on:`. The controller dispatches a batch of tasks whose dependencies are committed, whose files are disjoint, and whose crates' tests do not load each other's. Implementers run only their focused tests and do not commit. The controller commits each task on its own, reviews it on exactly that commit, and runs the full suite once per batch.
- **Task 1 is a gate.** If its real-keychain round-trip fails, stop: H5.1 is dropped by a spec amendment, Task 7 shrinks to H5.2, and every other task stands.
- **Task 12 is a gate for itself.** If compact rows miss the 1 s budget, stop and report before anything larger.
- **Outside the sandbox:** `openpty`, Unix-socket binding and keychain creation fail with EPERM inside it. Each task names the tests that need it off; run exactly those outside.
- **New crates:** none.
- **Format and lint, in every task:** `cargo fmt --all` first; clippy passes with `-D warnings` both with and without `--features tagteam/test-support`.
- **Test isolation (core §15.1):** tests never reach the real HOME, network, Keychain, `claude`, or a daemon the person runs.
- **Commits:** one imperative sentence in the repository's style, no prefix, no attribution lines.

---

### Task 1: Prove the `-w` text round-trip on a real keychain (H5.1 gate)

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-provider/src/security.rs` (add `escape_text` right after `check_name`, :396-404)
- Modify: `crates/tagteam-provider/tests/real_keychain.rs` (`TempKeychain`, :11-40, snapshots and restores the user's keychain search list; new helpers and the two gate tests)
- Test: `crates/tagteam-provider/src/security.rs` `mod tests` (:582-); `crates/tagteam-provider/tests/real_keychain.rs` (gated `#![cfg(all(target_os = "macos", feature = "real_keychain"))]`, as today)

**Interfaces:**
- Consumes: `security::{SecurityCli, ProcessRunner, Runner, RunResult, SECURITY, LINE_LIMIT}` (all `pub` today); `Keychain::{find, delete_service}`.
- Produces: `pub(crate) fn escape_text(credential: &[u8]) -> Option<String>` in `security.rs` (the brief's fixed interface). In `tests/real_keychain.rs`: `fn text_cases() -> Vec<(&'static str, String)>`, `fn full_line_text(keychain: &Path) -> String` and `const W_SERVICE: &str = "tagteam-test-w-text"`, which Task 7 reuses.

Why the gate escapes with its own copy of the rule: `escape_text` is `pub(crate)` (fixed), so `tests/real_keychain.rs` cannot call it, and CI's macOS step runs only `--test real_keychain`, so a feature-gated test inside the library would never run in CI. The gate therefore tests what `security -i`'s parser makes of the rule; `escape_text`'s unit tests pin that the function is exactly that rule; Task 7's `upsert` test closes the loop through the real function, in CI.

- [ ] **Step 1: Write the failing tests**

  In `security.rs` `mod tests`:
  ```rust
  #[test]
  fn escape_text_escapes_only_backslashes_and_quotes() {
      assert_eq!(
          escape_text(br#"{"a":"b\c"}"#).as_deref(),
          Some(r#"{\"a\":\"b\\c\"}"#)
      );
      // A JSON escape sequence is two characters of text; its backslash is escaped like any other.
      assert_eq!(
          escape_text(br#""x\n\u00e9""#).as_deref(),
          Some(r#"\"x\\n\\u00e9\""#)
      );
      let untouched = "é 日本 🔑 # $ % ' \t end";
      assert_eq!(escape_text(untouched.as_bytes()).as_deref(), Some(untouched));
  }

  #[test]
  fn escape_text_refuses_nul_cr_lf_and_bytes_that_are_not_utf8() {
      for bytes in [&b"a\0b"[..], b"a\rb", b"a\nb", &[0xff, 0xfe], &[0xc3]] {
          assert_eq!(escape_text(bytes), None, "{bytes:?}");
      }
  }
  ```

  In `tests/real_keychain.rs`:
  - `TempKeychain` becomes `struct TempKeychain(PathBuf, TempDir, Vec<String>, MutexGuard<'static, ()>)` (keep `.0` as the path). The search list is shared by the whole user session and cargo runs this file's tests on parallel threads, so a fixture's whole life (snapshot, create, use, delete, restore) is serialized: `new()` first takes a file-wide `static SEARCH_LIST: Mutex<()>` (recovering a poisoned lock with `into_inner`), and the guard is the last field, so it drops after `Drop::drop` has restored the list. `new()` then reads `security list-keychains -d user` (one quoted path per line, trimmed of whitespace and `"`, as `xtask/src/compat/keychain.rs::parse_search_list` does) and asserts it succeeded and is non-empty (an empty list is a failed read, never a snapshot). `Drop` runs `delete-keychain` as today, then, best effort and without asserting (a panic in `Drop` while unwinding aborts), re-reads the list and, when it differs from the snapshot, runs `security list-keychains -d user -s <snapshot…>`.
  - Helpers:
    ```rust
    const W_SERVICE: &str = "tagteam-test-w-text";

    /// `security::escape_text`'s rule (H5.1), restated here because the function is crate-private.
    fn escaped(text: &str) -> String {
        text.replace('\\', "\\\\").replace('"', "\\\"")
    }

    /// The `-i` line `upsert` writes for text from Task 7 on, with the test keychain as its last word.
    fn text_line(keychain: &Path, service: &str, account: &str, text: &str) -> String {
        format!(
            "add-generic-password -U -a \"{account}\" -s \"{service}\" -w \"{}\" \"{}\"\n",
            escaped(text),
            keychain.display()
        )
    }

    /// Writes `text` through `security -i`. `security -i` exits 0 even when a command fails, so an
    /// empty stderr is part of success. Returns the line's length.
    fn write_text_line(keychain: &Path, service: &str, account: &str, text: &str) -> usize {
        let line = text_line(keychain, service, account, text);
        assert!(line.len() <= LINE_LIMIT, "{account}: {} bytes", line.len());
        match ProcessRunner.run(SECURITY, &["-i".to_owned()], Some(line.as_bytes()), Duration::from_secs(5)) {
            RunResult::Exited { code: 0, stderr, .. } if stderr.is_empty() => line.len(),
            RunResult::Exited { code, stderr, .. } => panic!(
                "{account}: security -i rc {code}: {}", String::from_utf8_lossy(&stderr)
            ),
            other => panic!("{account}: {other:?}"),
        }
    }
    ```
  - `text_cases()` returns `(account, text)` pairs, each a JSON credential (Review Focus line 1):
    `("quote-backslash", r#"{"claudeAiOauth":{"accessToken":"a\"b\\c"}}"#)`,
    `("json-escapes", r#"{"n":"line\nbreak","e":"caf\u00e9","t":"\t"}"#)` (backslash sequences as text),
    `("raw-non-ascii", "{\"e\":\"café\",\"cjk\":\"日本語\",\"emoji\":\"🔑\"}")`,
    `("shell-chars", r##"{"x":"#hash $HOME %s 'single' a  b"}"##)`,
    `("raw-tab", "{\"t\":\"a\tb\"}")`.
  - `full_line_text(keychain)`: the text `{"claudeAiOauth":{"accessToken":"sk-ant-oat01-\"q\"\\","refreshToken":"rt"},"pad":"<x…>"}` with the run of `x` sized so that `text_line(keychain, W_SERVICE, "full-line", &text).len() == LINE_LIMIT` exactly (compute it with an empty pad first); assert that equality inside the helper.
  - The gate tests:
    ```rust
    #[test]
    fn a_w_text_line_round_trips_every_h5_1_case_byte_for_byte() {
        let kc = TempKeychain::new();
        let k = SecurityCli::with_runner(Box::new(ProcessRunner), Some(kc.0.clone()));
        let mut cases = text_cases();
        cases.push(("full-line", full_line_text(&kc.0)));
        for (account, text) in &cases {
            write_text_line(&kc.0, W_SERVICE, account, text);
            // `find` applies A.3's `-g` disambiguation to whatever `-w` renders as hex.
            assert_eq!(k.find(W_SERVICE, account).present().unwrap(), text.as_bytes(), "{account}");
        }
    }

    #[test]
    fn a_w_text_line_with_dash_u_updates_an_existing_item_in_place() {
        let kc = TempKeychain::new();
        let k = SecurityCli::with_runner(Box::new(ProcessRunner), Some(kc.0.clone()));
        let (first, second) = (r#"{"g":"1 \"a\""}"#, r#"{"g":"2 \\b é"}"#);
        write_text_line(&kc.0, W_SERVICE, "id", first);
        write_text_line(&kc.0, W_SERVICE, "id", second);
        assert_eq!(k.find(W_SERVICE, "id").present().unwrap(), second.as_bytes());
        assert_eq!(k.delete_service(W_SERVICE).unwrap(), 1, "one item, updated in place");
    }
    ```

- [ ] **Step 2: Run them to verify they fail**

  `cargo test -p tagteam-provider --lib security::tests::escape_text` → compile error: cannot find function `escape_text` in this scope.
  The two keychain tests are a probe of `security`'s parser, not of tagteam code, so they have no failing phase; they run in Step 4.

- [ ] **Step 3: Implement**

  `pub(crate) fn escape_text(credential: &[u8]) -> Option<String>`: `None` unless `std::str::from_utf8` succeeds and the text contains none of `'\0'`, `'\r'`, `'\n'`; otherwise the text with `\` written `\\` and `"` written `\"` (backslashes first, or one pass). Mark it `#[cfg_attr(not(test), expect(dead_code, reason = "upsert writes -w text from Task 7"))]` so clippy `-D warnings` passes until Task 7, which removes the attribute (an unfulfilled `expect` then fails clippy, so it cannot be forgotten).

- [ ] **Step 4: Run them to verify they pass — this is the gate**

  - Sandboxed: `cargo test -p tagteam-provider --lib security::tests` → PASS.
  - The gate, **outside the sandbox** (Bash with `dangerouslyDisableSandbox: true`): `security create-keychain` fails inside it; a first sandboxed run shows that failure, which is the evidence the house rule asks for:
    `cargo test -p tagteam-provider --locked --features real_keychain --test real_keychain`
    → PASS, 6 tests (the 4 existing ones and the 2 new ones). Afterwards `security list-keychains -d user` lists exactly what it listed before.
  - Before calling a failure, rule out the read side: write the failing case with today's `k.upsert` (the `-X` hex line) on a fresh `TempKeychain` and `find` it. If that fails the same way, the fault is in reading (A.3), which H5.1 does not change; report it, but it does not decide the gate.
  - **If either new test fails** (anything other than keychain creation failing), STOP the plan before Task 7. Report the failing case and its stderr to Michael for a spec amendment: H5.1 is dropped, H5.2 and H5.3 are kept. Task 7 then shrinks to H5.2 alone (the `-X` hex line, argv with the WARN past the limit), `escape_text` and its tests are removed, and Tasks 8 and 9 are unaffected.

- [ ] **Step 5: Commit**

  `crates/tagteam-provider/src/security.rs`, `crates/tagteam-provider/tests/real_keychain.rs` — "Prove that security -i round-trips credentials written as -w text"

---

### Task 2: Release pipeline: no build cache, attestations, cargo-deny (H3)

**Depends on:** none

**Files:**
- Modify: `dist-workspace.toml` (add three keys after `global-artifacts-jobs`, line 23; reword the comment on line 25; in `[dist.github-action-commits]`, lines 26-30, drop `rust-cache` and add `actions/attest`)
- Modify: `.github/workflows/release.yml` (regenerated by `dist generate`, never hand-edited)
- Modify: `.github/workflows/ci.yml` (add a `schedule` trigger at lines 3-7, an `if` on the `check` job at line 21, and a new `deny` job after it)
- Create: `deny.toml`
- Modify: `Cargo.lock` (`cargo update -p rustls`, see Step 3)
- Modify: `CONTRIBUTING.md` (lines 36-45 and 173: name the `deny` job among the checks CI runs)
- Test: none in Rust. The checks are the commands in Steps 1, 2 and 4.

Not in this task: `SECURITY.md` (H3's `gh attestation verify` text and the removal of the RUSTSEC-2023-0071 note at lines 74-76 belong to Task 20, which owns that file) and `Cargo.toml:25`'s comment about the same advisory (leave it).

**Interfaces:**
- Consumes: nothing.
- Produces: a `deny` job and an `on.schedule` trigger in `ci.yml`. Task 19 adds a second cache-free job to the same file and gives it the same `if: github.event_name != 'schedule'` as `check`.

**Facts established while writing this plan (re-verify the two SHAs, they are live data):**
- dist 0.33.0 at `~/.local/share/mise/installs/cargo-dist/0.33.0/cargo-dist-aarch64-apple-darwin/dist`. It shells out to `cargo`, so put `~/.cargo/bin` on `PATH`; `mise exec` would try to install Rust. If mise prints "Config files in .../mise.toml are not trusted" in a fresh worktree, that is noise unless dist then fails; trusting the file is Michael's call.
- Baseline: on 5c9a114, `dist generate --check` exits 0 and `dist generate` changes nothing.
- `actions/attest` latest release v4.2.2 (2026-08-04), tag is lightweight, commit `1e69f48acb82d1966a394da916b4c1698aa569d6`. dist emits `uses: actions/attest@<that SHA>` once it is in `[dist.github-action-commits]`.
- `EmbarkStudios/cargo-deny-action` latest release v2.1.1 (2026-07-13); the tag is annotated, its commit is `3c6349835b2b7b196a839186cb8b78e02f7b5f25` (`git ls-remote --tags https://github.com/EmbarkStudios/cargo-deny-action 'v2.1.1*'` shows the `^{}` line; `v2` points at the same commit). It is a Docker action (Linux runners only; it carries its own Rust and cargo-deny, restores no Actions cache). Its default `arguments` is `--all-features`, which is wanted here. Re-run `git ls-remote` for both repositories first; if a newer release exists, use it, and keep the SHA comment `# vX.Y.Z`.
- cargo-deny 0.20.2 is the current release. It is not installed here and not in `mise.toml` (which pins only `cargo-dist`; adding a tool there is Michael's decision, so this plan does not).
- **Finding, not in the spec:** the RustSec database (clone of 2026-10-17) lists four vulnerability advisories against `rustls 0.23.45`, the locked version: RUSTSEC-2026-0346, -0347, -0348 and -0349 (all patched in `>= 0.23.46`; published 2026-10-14). `ureq ~3.4` takes rustls only through `platform-verifier`. `cargo update -p rustls --dry-run` resolves to 0.23.46 and changes nothing else. Without that update the new `deny` job fails on its first run. The only other hits for the lock are RUSTSEC-2023-0071 (`rsa 0.9.10`, ignored below) and RUSTSEC-2025-0007 (`ring`, withdrawn 2025-02-22, which cargo-deny skips). That scan was a script over the advisory database, not cargo-deny itself, which could not be downloaded in the planning sandbox.

- [ ] **Step 1: Write the failing checks**
  - Edit `dist-workspace.toml`. After `global-artifacts-jobs`:
    ```toml
    # No Actions cache in release builds (spec H3)
    cache-builds = false
    # Attest the release artifacts when the release is hosted, which happens on tags only
    github-attestations = true
    github-attestations-phase = "host"
    ```
    In `[dist.github-action-commits]` delete the `"swatinem/rust-cache"` line and add `"actions/attest" = "1e69f48acb82d1966a394da916b4c1698aa569d6"`. Change the comment above the table to name the pinned versions: checkout v7.0.1, upload-artifact v7.0.1, download-artifact v8.0.1, attest v4.2.2.
  - Create `deny.toml` (verbatim):
    ```toml
    # cargo-deny: advisories only (spec H3). ci.yml's `deny` job runs `cargo deny check advisories`.

    [advisories]
    # A vulnerability advisory always fails the check, for any crate in the graph.
    # An unsound advisory fails it for any crate in the graph too.
    unsound = "all"
    # An unmaintained notice fails it only for the workspace's direct dependencies: the
    # transitive trees under ratatui and age are not tagteam's to replace.
    unmaintained = "workspace"
    # A yanked version warns.
    yanked = "warn"
    ignore = [
        { id = "RUSTSEC-2023-0071", reason = "age's `ssh` feature pulls in `rsa`, whose RSA decryption has a timing side channel. tagteam refuses RSA identity files and `ssh-rsa` recipients (`parse_identity_file` and `parse_recipient` in crates/tagteam-engine/src/transfer.rs), so it never decrypts with an RSA key." },
    ]
    ```
    `unsound` defaults to `"workspace"` in cargo-deny, so the explicit `"all"` is what implements "any crate". An advisory that falls inside this policy and cannot be fixed on this branch gets its own `{ id, reason }` entry here and is listed in the MR; none is expected (Step 3).
  - In `ci.yml`, add the `deny` job (below) and the schedule. Add under `on:` (after `workflow_dispatch:`)
    ```yaml
      schedule:
        - cron: "41 6 * * 1"
    ```
    and put `if: github.event_name != 'schedule'` on the `check` job (a weekly run exists for the advisory database, not for the build matrix). The new job, after `check`:
    ```yaml
      deny:
        name: deny (advisories)
        runs-on: ubuntu-latest
        steps:
          - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
            with:
              persist-credentials: false
          - uses: EmbarkStudios/cargo-deny-action@3c6349835b2b7b196a839186cb8b78e02f7b5f25 # v2.1.1
            with:
              command: check advisories
    ```
    No `Swatinem/rust-cache` step in it (spec: a poisoned cache must not be able to make it pass); a comment above the job says so. The workflow-level `permissions: contents: read` already applies.
  - Add the same triggers to nothing else; `cc-drift.yml` is unchanged (spec).

- [ ] **Step 2: Run them to verify they fail**
  - `PATH=$HOME/.cargo/bin:$PATH ~/.local/share/mise/installs/cargo-dist/0.33.0/cargo-dist-aarch64-apple-darwin/dist generate --check` (run from the worktree root, outside the sandbox only if the sandbox blocks `cargo metadata`). Expected: non-zero exit, saying `release.yml` is out of date with `dist-workspace.toml`.
  - cargo-deny, to see the rustls failure first: it is not installed, and ad hoc installs are not allowed, so run it from CI on the branch push, or locally with `mise x cargo-deny@0.20.2 -- cargo deny --manifest-path <worktree>/Cargo.toml --config <worktree>/deny.toml check advisories`, invoked from a directory outside the worktree so mise does not read the repository's `rust-toolchain.toml`. It fetches the advisory database from github.com, so it needs the network (outside the sandbox). Expected: 4 errors, RUSTSEC-2026-0346/0347/0348/0349 on `rustls 0.23.45`, and no error for RUSTSEC-2023-0071. If the local run is not possible, say so in the MR; the CI job is then the check.

- [ ] **Step 3: Implement**
  - `dist generate` (same invocation, no `--check`). Do not touch `release.yml` by hand.
  - `cargo update -p rustls` (needs the network; outside the sandbox if it is refused). Expected: `Updating rustls v0.23.45 -> v0.23.46`, nothing else in `git diff Cargo.lock`. If the diff shows more, stop and report it.
  - `CONTRIBUTING.md`: in the check list (lines 36-45) add one sentence that CI also runs `cargo deny check advisories` (the `deny` job, Linux only, run in the cloud rather than by `cargo` locally), and add "and the `deny` job" to the "CI must be green" bullet at line 173.

- [ ] **Step 4: Run them to verify they pass**
  - `dist generate --check` exits 0. `git diff --stat .github/workflows/release.yml` shows `9 insertions, 4 deletions`, and the diff is exactly (verified in a scratch copy at 5c9a114 with the pins above):
    1. `build-local-artifacts`: the three-line `swatinem/rust-cache` step (`uses`, `with: key: ${{ join(matrix.targets, '-') }}`, `cache-provider: ${{ matrix.cache_provider }}`) is gone (around line 130).
    2. `host` job: a new `permissions:` block with `"attestations": "write"`, `"contents": "write"`, `"id-token": "write"` after `outputs:`; the job runs only when `needs.plan.outputs.publishing == 'true'`, that is on tags, so pull-request runs get no `id-token`.
    3. `host` job: a step `name: Attest` / `uses: actions/attest@1e69f48acb82d1966a394da916b4c1698aa569d6` / `with: subject-path: |\n artifacts/*` immediately before `Create GitHub Release`.
    Any other line in that diff is a finding to report in the MR, not to keep. (The `cargo-dist-cache` artifact upload and download steps stay; they carry the dist binary between jobs and are not an Actions cache.)
  - `dist plan` still prints the announcement for all four targets and exits 0.
  - `git diff --stat` for the task: `dist-workspace.toml`, `release.yml`, `ci.yml`, `deny.toml`, `Cargo.lock` (2 lines: version and checksum of rustls), `CONTRIBUTING.md`.
  - `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support` still pass on the new lock (the usual tests; the pty and Unix-socket ones outside the sandbox as always).
  - The cargo-deny run from Step 2 now exits 0 (the only advisory line left is the ignored RUSTSEC-2023-0071, shown as ignored). The `deny` job must be green on the pushed branch, and `ci.yml` must still parse: `actionlint` is not required, but `git diff` on the file must show only the three edits.

- [ ] **Step 5: Commit** — `dist-workspace.toml`, `.github/workflows/release.yml`, `.github/workflows/ci.yml`, `deny.toml`, `Cargo.lock`, `CONTRIBUTING.md`: "Build releases without a cache, attest them, and check advisories in CI". Two commits are fine if the controller prefers (dist and workflow, then deny.toml with the lock update); the MR lists the rustls update as a finding fixed here.

---

### Task 3: One bounded runner, with `ExitWatch`, in `process.rs` (H4)

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-provider/src/process.rs`
  - `POLL` and `DRAIN_GRACE` (198-202): `POLL` becomes `SLICE`; `DRAIN_GRACE` becomes `pub(crate)`.
  - `run_captured` and `capture` (220-265), `wait_for` (267-290) and `run_discarded` (292-316) are replaced by `ExitWatch`/`Watched`, `Output`, `bounded`, `run_bounded`, and `Running`/`Spawned`/`wait_running`. `run_captured` and `run_discarded` stay as thin wrappers.
  - `kill_group` (318-332) no longer reaps, and returns whether a kill was sent.
  - `collect` (362-383) takes `Option<&Cancel>`.
  - `mod tests` (565-1275): a local `capture` helper, so the existing tests stay unmodified, plus the new tests.
- Modify: `crates/tagteam-provider/src/lib.rs` (65-69: add `ExitWatch`, `Watched` and `run_bounded` to the `pub use process::{…}` list).
- Test: `crates/tagteam-provider/src/process.rs` (`mod tests`).

**Interfaces:**
- Consumes: nothing new. `libc` is already a normal dependency of `tagteam-provider` (`Cargo.toml` `[dependencies]`, 0.2.189 locked). Its apple and linux_like modules declare `waitid`, `P_PID`, `WEXITED`, `WNOWAIT` and `WNOHANG`, and `term.rs:162-176` (`child_is_stopped`) already calls `waitid` with `WNOWAIT` on both. Scratch probes on this Mac confirmed four things:
  - a waiter thread's `waitid(WEXITED|WNOWAIT)` returns as the child exits and leaves it reapable;
  - `killpg` fails with `EPERM` on a group whose only member is the exited leader;
  - `kill` of that zombie succeeds, and `waitpid` still returns its exit code;
  - `waitid` on this process's own pid fails with `ECHILD`.
- Produces (the first three are re-exported at the crate root):
  - `pub struct ExitWatch` with `impl ExitWatch { pub fn start(pid: u32) -> ExitWatch; pub fn wait(&mut self, timeout: Duration) -> Watched }`.
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub enum Watched { Exited, Pending, Failed(String) }`.
  - `pub fn run_bounded(spec: &SpawnSpec, stdin: Option<Vec<u8>>, timeout: Duration, cancel: Option<&Cancel>) -> Captured`.
  - Crate-private, for Task 4's adapter: `pub(crate) enum Output { Pipe, Null }`, `pub(crate) const DRAIN_GRACE: Duration` (2 s, unchanged), and `pub(crate) fn bounded(spec: &SpawnSpec, stdin: Option<Vec<u8>>, output: Output, timeout: Duration, cancel: Option<&Cancel>, grace: Duration) -> Captured`.
  - Unchanged: `run_captured`, `run_discarded`, `Captured`, `Discarded`, `ProcessSpawner`, `SystemSpawner`, `ScriptedSpawner`, `drain` and its `#[cfg(test)]` `DRAINS` counter.

- [ ] **Step 1: Write the failing tests** in `process.rs`'s `mod tests`. These existing real-process tests must pass unmodified on the new runner:
  - `a_timeout_kills_the_child_s_whole_group` (:790);
  - `a_cancel_kills_the_child_s_whole_group_and_names_the_signal` (:810);
  - `a_cancel_while_the_output_drains_is_an_interruption` (:876);
  - `output_held_open_past_the_grace_is_never_half_reported` (:863);
  - `a_captured_child_leads_a_process_group_of_its_own`;
  - `a_token_already_set_spawns_nothing`;
  - `a_discarded_child_is_never_given_a_pipe_to_fill_or_to_hold` (the `DRAINS` count);
  - `a_discarded_child_is_killed_with_its_group_on_the_timeout_or_a_cancel`.

  Helpers (test code):

```rust
/// The runner as `capture` was, with its grace a parameter; the tests above call it.
fn capture(spec: &SpawnSpec, timeout: Duration, cancel: &Cancel, grace: Duration) -> Captured {
    bounded(spec, None, Output::Pipe, timeout, Some(cancel), grace)
}

/// `pid`, a child of this process, is not reaped yet (running, or a zombie): `waitid` answers
/// for it, and fails with ECHILD once it is reaped.
fn unreaped(pid: u32) -> bool {
    // SAFETY: `info` is a zeroed siginfo_t that waitid fills in; WNOHANG and WNOWAIT make it
    // return at once and reap nothing.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) == 0
    }
}

/// `f`'s result from a thread of its own, or a panic once `limit` has passed: a wait that only
/// a tick or a deadline would end fails here instead of hanging.
fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit).expect("it did not return in time")
}

#[derive(Debug, PartialEq)]
enum Did {
    Watch(Duration),
    Kill,
    Reap,
}

/// A child whose exit-watch answers are scripted, recording what the wait did to it (the role
/// `security.rs`'s `FakeChild` had).
struct FakeRun {
    answers: VecDeque<Watched>,
    killable: bool,
    did: Vec<Did>,
}

impl FakeRun {
    fn new(answers: Vec<Watched>) -> Self {
        Self { answers: answers.into(), killable: true, did: Vec::new() }
    }
    fn unkillable(mut self) -> Self {
        self.killable = false;
        self
    }
}

impl Running for FakeRun {
    fn watch(&mut self, timeout: Duration) -> Watched {
        self.did.push(Did::Watch(timeout));
        self.answers.pop_front().expect("unexpected extra watch")
    }
    fn kill_group(&mut self) -> bool {
        self.did.push(Did::Kill);
        self.killable
    }
    fn reap(&mut self) -> io::Result<ExitStatus> {
        self.did.push(Did::Reap);
        Ok(ExitStatus::from_raw(3 << 8))
    }
}
```

  Tests on the `ExitWatch`, against real children (each takes `fork_guard()`):

```rust
#[test]
fn an_exit_watch_sees_the_exit_and_leaves_the_child_unreaped() {
    let _fork = fork_guard();
    let mut child = Command::new("/bin/sh").args(["-c", "exit 3"]).spawn().unwrap();
    let pid = child.id();
    let mut watch = ExitWatch::start(pid);
    assert_eq!(watch.wait(Duration::from_secs(10)), Watched::Exited);
    assert!(unreaped(pid), "WNOWAIT: the pid still names the zombie, and its group");
    assert_eq!(watch.wait(Duration::ZERO), Watched::Exited, "Exited stays Exited");
    assert_eq!(child.wait().unwrap().code(), Some(3), "the owner reaps it, status and all");
    assert!(!unreaped(pid));
}

#[test]
fn an_exit_watch_on_a_running_child_is_pending_until_it_ends() {
    let _fork = fork_guard();
    let mut child = Command::new("/bin/sh").args(["-c", "sleep 30"]).spawn().unwrap();
    let mut watch = ExitWatch::start(child.id());
    assert_eq!(watch.wait(Duration::from_millis(50)), Watched::Pending);
    child.kill().unwrap();
    assert_eq!(watch.wait(Duration::from_secs(10)), Watched::Exited);
    child.wait().unwrap();
}

#[test]
fn an_exit_watch_on_a_pid_that_is_not_a_child_fails() {
    // waitid: ECHILD. This process is not its own child.
    let mut watch = ExitWatch::start(std::process::id());
    assert!(matches!(watch.wait(Duration::from_secs(10)), Watched::Failed(_)));
}
```

  Tests on the waiting algorithm, deterministic through the `Running` seam. Four of them replace `security.rs`'s `FakeChild` tests, which Task 4 deletes:

```rust
#[test]
fn without_a_token_the_wait_asks_the_watch_once_for_the_whole_timeout() {
    // H4: no polling sleep; the exit wakes the one wait.
    let mut run = FakeRun::new(vec![Watched::Exited]);
    let status = wait_running(&mut run, Duration::from_secs(60), None).unwrap();
    assert_eq!(status.code(), Some(3));
    assert!(
        matches!(run.did.as_slice(), [Did::Watch(d), Did::Reap] if *d > Duration::from_secs(59)),
        "{:?}",
        run.did
    );
}

#[test]
fn under_a_token_the_wait_looks_at_it_every_10_ms_and_never_kills_a_child_that_exits() {
    // Replaces security.rs's `a_child_that_exits_in_time_is_left_alone`.
    let mut run = FakeRun::new(vec![Watched::Pending, Watched::Pending, Watched::Exited]);
    let status = wait_running(&mut run, Duration::from_secs(60), Some(&Cancel::new())).unwrap();
    assert_eq!(status.code(), Some(3));
    assert_eq!(run.did.len(), 4, "{:?}", run.did);
    assert!(run.did[..3].iter().all(|d| matches!(d, Did::Watch(t) if *t <= SLICE)));
    assert_eq!(run.did[3], Did::Reap);
}

#[test]
fn a_set_token_kills_the_group_then_waits_for_the_watch_then_reaps() {
    let cancel = Cancel::new();
    cancel.request(15);
    let mut run = FakeRun::new(vec![Watched::Pending, Watched::Exited]);
    assert_eq!(
        wait_running(&mut run, Duration::from_secs(60), Some(&cancel)).unwrap_err(),
        Captured::Interrupted(15)
    );
    assert_eq!(run.did[1..], [Did::Kill, Did::Watch(Duration::MAX), Did::Reap]);
}

#[test]
fn a_deadline_kills_the_group_then_waits_for_the_watch_then_reaps() {
    // Replaces security.rs's `a_child_past_its_deadline_is_killed_and_reaped`.
    let mut run = FakeRun::new(vec![Watched::Pending, Watched::Exited]);
    assert_eq!(wait_running(&mut run, Duration::ZERO, None).unwrap_err(), Captured::TimedOut);
    assert_eq!(
        run.did,
        [Did::Watch(Duration::ZERO), Did::Kill, Did::Watch(Duration::MAX), Did::Reap]
    );
}

#[test]
fn a_child_no_kill_reaches_is_neither_waited_for_nor_reaped() {
    // Replaces security.rs's `a_child_that_cannot_be_killed_is_not_waited_for`: a wait or a
    // reap would block for as long as the child runs.
    let mut past = FakeRun::new(vec![Watched::Pending]).unkillable();
    assert_eq!(wait_running(&mut past, Duration::ZERO, None).unwrap_err(), Captured::TimedOut);
    assert_eq!(past.did, [Did::Watch(Duration::ZERO), Did::Kill]);
    let mut failed = FakeRun::new(vec![Watched::Failed("boom".into())]).unkillable();
    let got = wait_running(&mut failed, Duration::from_secs(60), None);
    assert!(matches!(&got, Err(Captured::SpawnFailed(m)) if m.contains("boom")), "{got:?}");
    assert_eq!(failed.did[1..], [Did::Kill]);
}

#[test]
fn a_failed_watch_kills_and_reaps_the_child_and_is_a_spawn_failure() {
    // Replaces security.rs's `a_failed_try_wait_kills_and_reaps_the_child`.
    let mut run = FakeRun::new(vec![Watched::Failed("boom".into())]);
    let got = wait_running(&mut run, Duration::from_secs(60), None);
    assert!(matches!(&got, Err(Captured::SpawnFailed(m)) if m.contains("boom")), "{got:?}");
    assert_eq!(run.did[1..], [Did::Kill, Did::Reap]);
}
```

  Tests on the runner, against real children:

```rust
#[test]
fn a_child_s_exit_ends_a_bounded_run_without_waiting_out_its_timeout() {
    // H4: with no token the one wait is to the deadline, an hour away; the exit ends it.
    let _fork = fork_guard();
    let got = within(Duration::from_secs(10), || {
        run_bounded(&sh("exit 4"), None, Duration::from_secs(3600), None)
    });
    assert!(matches!(got, Captured::Exited { code: Some(4), .. }), "{got:?}");
}

#[test]
fn stdin_reaches_a_bounded_child_whole() {
    // 200 KB, past a pipe's buffer: the writer and the drains run beside the wait.
    let _fork = fork_guard();
    let input: Vec<u8> = (0..200_000u32).map(|i| b'a' + (i % 26) as u8).collect();
    let got = run_bounded(&sh("cat"), Some(input.clone()), Duration::from_secs(10), None);
    let Captured::Exited { code: Some(0), stdout, .. } = got else { panic!("cat did not run") };
    assert!(stdout == input, "{} bytes came back", stdout.len());
}

#[test]
fn a_cancel_interrupts_a_bounded_run_fed_on_stdin_and_kills_its_group() {
    let _fork = fork_guard();
    let d = tempfile::tempdir().unwrap();
    let marker = d.path().join("alive");
    let cancel = Cancel::new();
    let signaller = cancel.clone();
    let sender = thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        signaller.request(15);
    });
    let started = Instant::now();
    let got = run_bounded(
        &with_grandchild(&marker),
        Some(b"never read".to_vec()),
        Duration::from_secs(30),
        Some(&cancel),
    );
    sender.join().unwrap();
    assert_eq!(got, Captured::Interrupted(15));
    assert!(started.elapsed() < Duration::from_secs(5));
    thread::sleep(Duration::from_millis(1500));
    assert!(!marker.exists(), "the grandchild died with its group");
}

#[test]
fn a_child_killed_at_its_deadline_is_reaped_before_the_run_returns() {
    let _fork = fork_guard();
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("pid");
    let script = sh(&format!("echo $$ > '{}'; sleep 30", file.display()));
    assert_eq!(run_bounded(&script, None, Duration::from_millis(500), None), Captured::TimedOut);
    assert!(!unreaped(pid_in(&file) as u32), "no zombie is left behind");
}

#[test]
fn a_zombie_leader_s_group_is_killed_through_the_kill_fallback() {
    // H4, H14: the leader has exited and nothing else is in its group. macOS refuses killpg
    // (EPERM); the kill of the zombie is what goes, and the child is still reaped with its code.
    let _fork = fork_guard();
    let mut child = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id();
    assert_eq!(ExitWatch::start(pid).wait(Duration::from_secs(10)), Watched::Exited);
    #[cfg(target_os = "macos")]
    // SAFETY: killpg(2) with signal 0 only checks; `pid` is an unreaped child of this process.
    assert_eq!(unsafe { libc::killpg(pid as libc::pid_t, 0) }, -1, "killpg alone fails here");
    assert!(kill_group(&mut child), "the kill fallback was sent");
    assert_eq!(child.wait().unwrap().code(), Some(0));
}
```

- [ ] **Step 2: Run them to verify they fail.** `cargo test -p tagteam-provider --lib process::` fails to compile: it cannot find `ExitWatch`, `Watched`, `Running`, `wait_running`, `run_bounded`, `bounded`, `Output` or `SLICE`, and `kill_group` returns `()` where a `bool` is expected.

- [ ] **Step 3: Implement** in `process.rs`.
  - **`ExitWatch::start(pid)`** spawns a thread named `exit-watch` (`thread::Builder`).
    - The thread loops on `libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT)` with a zeroed `siginfo_t`, retrying `EINTR`.
    - It sends `Ok(())` on rc 0, or `Err(io::Error::last_os_error().to_string())`, on an `mpsc` channel, ignoring a receiver that is gone. It never reaps.
    - A thread that cannot be spawned makes the watch `Failed("could not start the exit watch: {e}")` at once.
    - The `SAFETY` comment follows `term.rs:164-166`.
  - **`wait(timeout)`** answers again any answer it already had: `Exited` and `Failed` are final. Otherwise it maps `recv_timeout(timeout)`:
    - `Ok(Ok)` → `Exited`;
    - `Ok(Err(e))` → `Failed(e)`;
    - `Timeout` → `Pending`;
    - `Disconnected` → `Failed("the exit watch ended without an answer")`.

    `Duration::MAX` waits until the answer comes (std's `recv_timeout` falls back to `recv` when the deadline overflows).
  - **`const SLICE: Duration = Duration::from_millis(10)`** is how often a wait under a cancel token looks at it (today's cancel latency). The exit itself wakes the wait at once.
  - **`kill_group(child: &mut Child) -> bool`** sends `killpg(SIGKILL)`. If that fails, it falls back to `child.kill()`. It returns whether either was sent, and **never reaps**. Its doc keeps today's `EPERM` explanation.
  - **The seam:**
    - `trait Running { fn watch(&mut self, timeout: Duration) -> Watched; fn kill_group(&mut self) -> bool; fn reap(&mut self) -> io::Result<ExitStatus>; }`.
    - `struct Spawned { child: Child, exit: ExitWatch }` implements it through `ExitWatch::wait`, `kill_group(&mut self.child)` and `self.child.wait()`.
  - **`fn wait_running(run: &mut dyn Running, timeout: Duration, cancel: Option<&Cancel>) -> Result<ExitStatus, Captured>`** pins this algorithm. The deadline is now + `timeout`. Loop:
    1. Ask `run.watch(slice)`. The slice is the time left to the deadline (saturating at zero). With a `cancel`, it is `min(SLICE, time left)`. Without one there is no polling at all: one wait runs to the deadline, and the exit ends it early.
    2. `Exited` → `run.reap()`. `Ok(status)` is the result; `Err(e)` is `SpawnFailed(e.to_string())`.
    3. `Failed(why)` → if `run.kill_group()` sent a kill, `run.reap()` (result ignored); then `SpawnFailed(why)`.
    4. `Pending` → if `cancel` holds a signal, stop with `Interrupted(signal)`; else if the deadline has passed, stop with `TimedOut`; else loop.
    5. **Stop:** if `run.kill_group()` sent a kill, `run.watch(Duration::MAX)` waits for the watcher (the killed leader becomes a zombie, so it answers), then `run.reap()`, then the outcome. If both kills failed, it returns the outcome at once, without waiting or reaping, as today. The watcher thread then ends by itself when the child exits.

    No path reaps before the watcher has answered, so the watcher never meets a reaped (or recycled) pid.
  - **`pub(crate) fn bounded(spec, stdin, output, timeout, cancel, grace) -> Captured`:**
    1. A `cancel` already holding a signal returns `Interrupted(signal)`, and nothing is spawned.
    2. Build the command with `command(spec)`: `process_group(0)`; stdin piped when `stdin.is_some()`, else null; stdout and stderr piped for `Output::Pipe`, null for `Output::Null`. A spawn error is `SpawnFailed(e.to_string())`.
    3. With stdin bytes, a detached thread runs `write_all` and drops the pipe, ignoring errors (`EPIPE` from a child that stopped reading).
    4. For `Output::Pipe`, `drain` each output pipe (as now).
    5. Run `wait_running(&mut Spawned { child, exit: ExitWatch::start(pid) }, timeout, cancel)`. An `Err(outcome)` returns it.
    6. For `Output::Pipe`, collect stdout, then stderr, against one `until = now + grace`, as today:
       - `collect(rx, until, cancel)` with a `cancel` waits in `SLICE`s and returns `Interrupted` when the token is set;
       - without one it does a single `recv_timeout`;
       - a pipe not closed by `until` is `TimedOut`.

       For `Output::Null` there is nothing to collect (empty vecs, and `From<Captured> for Discarded` drops them).
    7. Return `Exited { code: status.code(), signal: status.signal(), stdout, stderr }`.
  - `run_bounded(spec, stdin, timeout, cancel)` = `bounded(spec, stdin, Output::Pipe, timeout, cancel, DRAIN_GRACE)`.
  - `run_captured(spec, timeout, cancel)` = `run_bounded(spec, None, timeout, Some(cancel))`.
  - `run_discarded(spec, timeout, cancel)` = `bounded(spec, None, Output::Null, timeout, Some(cancel), DRAIN_GRACE).into()`.
  - Update `run_captured`'s doc ("polled every 10 ms" → "looked at every 10 ms while it waits; the exit is noticed at once").
  - `lib.rs`: re-export `ExitWatch`, `Watched` and `run_bounded`.

- [ ] **Step 4: Run them to verify they pass.**
  - `cargo test -p tagteam-provider --lib process::`: PASS, the eight existing tests listed in Step 1 included. All of these pass inside the sandbox (73 `process::` and `security::` lib tests passed there at 5c9a114).
  - `cargo test -p tagteam-provider`: `security.rs` still has its own runner until Task 4 and uses `crate::process::drain`, which stays `pub(crate)`.
  - Linux runs in CI's ubuntu leg (H15: `waitid` there is verified in libc's declarations, not yet run). The Docker repro in the session notes is optional.
  - No existing test changes.

- [ ] **Step 5: Commit** `crates/tagteam-provider/src/process.rs`, `crates/tagteam-provider/src/lib.rs`: "Run every bounded child through one runner that waits on its exit with waitid".

---

### Task 4: `security.rs` on the shared runner; interruptible reads (H4)

**Depends on:** 1, 3 (Task 1 also edits `security.rs`)

**Files:**
- Modify: `crates/tagteam-provider/src/security.rs`
  - Imports (1-14).
  - `RunResult` and its `Debug` (24-55): add `Interrupted(i32)`.
  - `Runner::run` (57-67): a `cancel` parameter.
  - 75-192 (`DRAIN_GRACE`, `Waitable`, `Waited`, `wait_for`, `reap`, `collect`, `ProcessRunner::run_bounded`): deleted, and replaced by the adapter `ProcessRunner::bounded`.
  - `impl Runner for ProcessRunner` (194-221).
  - `disambiguation_failed` (284-291).
  - `SecurityCli::run` (319-326): body only.
  - `describe`, `unreadable` and `failed` (366-394).
  - `upsert` (449-493), `delete` (495-503) and `delete_service` (517-550): the write guard.
  - `mod tests` (582-1439).
- Modify: `crates/tagteam-provider/src/cancel.rs`: the scope, after `Interrupted` (line 78), and its tests.
- Modify: `crates/tagteam-provider/src/read.rs` (`ReadError`, 24-45) and its tests.
- Modify: `crates/tagteam-provider/src/keychain.rs`:
  - the `Keychain::find` doc (106);
  - `locked_read` (176-178);
  - `FakeKeychain::find` (285-300);
  - tests (550-).
- Modify: `crates/xtask/src/compat/keychain.rs`: the test `Rec`'s `Runner::run` (713-728) takes the new parameter. Also update any other `impl Runner for` or direct `Runner::run`/`ProcessRunner.run` call that exists when this task runs (`rg -n 'impl Runner for|ProcessRunner\.run\(|\.run\(SECURITY' crates`). Today there is only `Rec`; Task 1 may add one to `crates/tagteam-provider/tests/real_keychain.rs`.
- Test: the `mod tests` of `security.rs`, `cancel.rs`, `read.rs` and `keychain.rs`.

**Interfaces:**
- Consumes: Task 3's `process::{bounded, Output, DRAIN_GRACE, SpawnSpec, Captured}`.
- Produces:
  - `RunResult::Interrupted(i32)`.
  - `Runner::run(&self, program: &str, args: &[String], stdin: Option<&[u8]>, timeout: Duration, cancel: Option<&Cancel>) -> RunResult`. `run_attached` is unchanged.
  - `SecurityCli::run(&self, args: Vec<String>, stdin: Option<&[u8]>) -> RunResult` (private) keeps its signature. It hands the runner `cancel::scoped()`.
  - `upsert`, `delete` and `delete_service` each start with `let _write = cancel::uninterruptible();`. Task 7 keeps that line first in `upsert`.
  - In `cancel.rs`:
    - `#[must_use] pub struct Interruptible`, `!Send` through `PhantomData<*const ()>`;
    - `pub fn interruptible(cancel: &Cancel) -> Interruptible`;
    - `pub fn scoped() -> Option<Cancel>`, the token of the innermost scope on this thread;
    - `pub(crate) fn uninterruptible() -> Interruptible`, a scope with no token.
  - In `read.rs`:
    - `ReadError::interrupted(what: impl Into<String>, signal: i32) -> ReadError`, with detail `"interrupted by signal {signal}"`;
    - `ReadError::locked(what: impl Into<String>, detail: impl Into<String>) -> ReadError`;
    - `pub fn interruption(&self) -> Option<i32>`;
    - `pub fn is_locked(&self) -> bool`.

    `ReadError::new` is neither. `what`, `detail` and `Display` are unchanged.
  - `FakeKeychain::find` answers `Unreadable(ReadError::interrupted("keychain", n))` inside a scope whose token holds `n`, counted as neither answered nor locked. `locked_read()` (the fakes' rc 36) is `ReadError::locked`.

- [ ] **Step 1: Write the failing tests.**

  In `security.rs`, `Scripted` gains `scoped: Arc<Mutex<Vec<bool>>>` (with `fn scoped(&self) -> Vec<bool>`) and the new `run` parameter:

```rust
fn run(&self, program: &str, args: &[String], stdin: Option<&[u8]>, _t: Duration,
       cancel: Option<&Cancel>) -> RunResult {
    self.calls.lock().unwrap().push((program.into(), args.to_vec(), stdin.map(<[u8]>::to_vec)));
    self.scoped.lock().unwrap().push(cancel.is_some());
    // As a real spawn does: a token already set runs nothing.
    if let Some(n) = cancel.and_then(Cancel::requested) {
        return RunResult::Interrupted(n);
    }
    self.results.lock().unwrap().pop_front().expect("unexpected extra call")
}
```

  Existing tests that change:
  - Deleted: `FakeChild`, its `impl Waitable`, its four tests, and `ExitStatusExt`, the one test import only they used (`io` stays for `pid_and_group`). Task 3's seam tests replace the tests, as listed there:
    - `a_failed_try_wait_kills_and_reaps_the_child`;
    - `a_child_past_its_deadline_is_killed_and_reaped`;
    - `a_child_that_cannot_be_killed_is_not_waited_for`;
    - `a_child_that_exits_in_time_is_left_alone`.
  - Call site only: in `a_real_process_reports_its_code_and_both_streams`, `a_grandchild_holding_the_pipes_cannot_hang_run` and `a_bounded_child_leads_a_process_group_of_its_own`, `ProcessRunner::run_bounded(p, a, i, t, g)` becomes `ProcessRunner::bounded(p, a, i, t, None, g)`.
  - Every other test is unchanged, `find_maps_return_codes` included: rc 36's detail text stays `"rc 36: boom"`.

  New tests:

```rust
#[test]
fn reads_take_the_scope_s_token_and_nothing_takes_one_outside_a_scope() {
    let s = Scripted::default()
        .then(ok(b"cafe\n")).then(og_verbatim("cafe")) // find, and its -g
        .then(ok(b"")).then(ok(b"")).then(ok(b""))     // exists, lock_state, service_has_items
        .then(ok(b"{}\n")).then(ok(b"")).then(ok(b"")).then(ok(b""));
    let k = cli(&s, None);
    let token = Cancel::new();
    {
        let _scope = cancel::interruptible(&token);
        assert!(k.find("svc", "acct").is_present());
        assert!(k.exists("svc", "acct").is_present());
        assert_eq!(k.lock_state(), LockState::Unlocked);
        assert!(matches!(k.service_has_items("svc"), Read::Present(true)));
    }
    assert!(k.find("svc", "acct").is_present());
    assert!(k.exists("svc", "acct").is_present());
    assert_eq!(k.lock_state(), LockState::Unlocked);
    assert!(matches!(k.service_has_items("svc"), Read::Present(true)));
    assert_eq!(s.scoped(), [true, true, true, true, true, false, false, false, false]);
}

#[test]
fn a_read_whose_scope_holds_a_signal_is_interrupted_and_counted_as_neither() {
    // H4, §14.1. Nothing is scripted: a call that ran would panic.
    let s = Scripted::default();
    let k = cli(&s, None);
    let token = Cancel::new();
    token.request(15);
    let _scope = cancel::interruptible(&token);
    let Read::Unreadable(e) = k.find("svc", "acct") else { panic!("not interrupted") };
    assert_eq!((e.interruption(), e.is_locked()), (Some(15), false));
    assert!(matches!(k.exists("svc", "acct"), Read::Unreadable(e) if e.interruption() == Some(15)));
    assert_eq!(k.lock_state(), LockState::Unknown);
    assert_eq!(k.read_counts().unwrap().tally(), ReadTally::default());
}

#[test]
fn a_cancel_set_before_a_write_stops_neither_the_write_nor_its_verify() {
    // H4, H14: upsert (its verify read included), delete and delete_service never take the
    // token, and the scope is whole again after each.
    let s = Scripted::default()
        .then(ok(b"")).then(ok(b"{}\n"))          // upsert's -i line, its verify read
        .then(rc(44))                              // delete
        .then(ok(b"")).then(rc(44)).then(rc(44)); // delete_service: one, none left, the probe
    let k = cli(&s, None);
    let token = Cancel::new();
    token.request(15);
    let _scope = cancel::interruptible(&token);
    k.upsert("svc", "acct", b"{}").unwrap();
    k.delete("svc", "acct").unwrap();
    assert_eq!(k.delete_service("svc").unwrap(), 1);
    assert_eq!(s.scoped()[..], [false; 6]);
    assert!(
        matches!(k.find("svc", "acct"), Read::Unreadable(e) if e.interruption() == Some(15)),
        "the next read is in the scope again"
    );
}

#[test]
fn rc_36_is_a_locked_read_and_a_timeout_or_any_other_rc_is_not() {
    let s = Scripted::default().then(rc(36)).then(RunResult::TimedOut).then(rc(1));
    let k = cli(&s, None);
    let locked = |r: Read<Vec<u8>>| match r {
        Read::Unreadable(e) => e.is_locked(),
        other => panic!("{other:?}"),
    };
    assert!(locked(k.find("s", "a")));
    assert!(!locked(k.find("s", "a")));
    assert!(!locked(k.find("s", "a")));
}

/// A shell that leaves a grandchild writing `marker` a second later, then waits 30 s.
fn with_grandchild(marker: &Path) -> Vec<String> {
    s(&["-c", &format!("(sleep 1; echo alive > '{}') & sleep 30", marker.display())])
}

#[test]
fn a_timeout_or_a_cancel_kills_a_security_child_s_whole_group() {
    // H4: `security`'s own runner killed the child alone and never saw the token.
    let _fork = crate::fork_guard();
    let d = tempfile::tempdir().unwrap();
    let (timed, cancelled) = (d.path().join("timed"), d.path().join("cancelled"));
    let r = ProcessRunner.run("/bin/sh", &with_grandchild(&timed), None, Duration::from_millis(200), None);
    assert!(matches!(r, RunResult::TimedOut), "{r:?}");
    let token = Cancel::new();
    let signaller = token.clone();
    let sender = thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        signaller.request(15);
    });
    let started = Instant::now();
    let r = ProcessRunner.run("/bin/sh", &with_grandchild(&cancelled), None, Duration::from_secs(30), Some(&token));
    sender.join().unwrap();
    assert!(matches!(r, RunResult::Interrupted(15)), "{r:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    thread::sleep(Duration::from_millis(1500));
    assert!(!timed.exists() && !cancelled.exists(), "both grandchildren died with their groups");
}

#[test]
fn stdin_reaches_a_security_child_and_a_signal_s_end_is_rc_minus_one() {
    let _fork = crate::fork_guard();
    let line = b"add-generic-password -U -a \"acct\" -s \"svc\" -X \"00\"\n";
    let r = ProcessRunner.run("/bin/sh", &s(&["-c", "cat"]), Some(line), Duration::from_secs(5), None);
    assert!(matches!(&r, RunResult::Exited { code: 0, stdout, .. } if stdout == line), "{r:?}");
    let r = ProcessRunner.run("/bin/sh", &s(&["-c", "kill -KILL $$"]), None, Duration::from_secs(5), None);
    assert!(matches!(r, RunResult::Exited { code: -1, .. }), "{r:?}");
}
```

  In `cancel.rs`:

```rust
#[test]
fn a_scope_sets_this_thread_s_token_until_it_is_dropped() {
    let token = Cancel::new();
    assert!(scoped().is_none());
    {
        let _scope = interruptible(&token);
        token.request(2);
        assert_eq!(scoped().and_then(|c| c.requested()), Some(2), "the token itself, shared");
    }
    assert!(scoped().is_none());
}

#[test]
fn nested_scopes_restore_the_one_they_replaced() {
    let (outer, inner) = (Cancel::new(), Cancel::new());
    outer.request(1);
    inner.request(15);
    let _a = interruptible(&outer);
    {
        let _b = interruptible(&inner);
        assert_eq!(scoped().and_then(|c| c.requested()), Some(15));
        {
            let _w = uninterruptible();
            assert!(scoped().is_none(), "a write sees no token");
        }
        assert_eq!(scoped().and_then(|c| c.requested()), Some(15));
    }
    assert_eq!(scoped().and_then(|c| c.requested()), Some(1));
}

#[test]
fn a_scope_belongs_to_the_thread_that_set_it() {
    let token = Cancel::new();
    let _scope = interruptible(&token);
    assert!(std::thread::spawn(|| scoped().is_none()).join().unwrap());
}
```

  In `read.rs`:

```rust
#[test]
fn an_error_says_whether_it_was_a_lock_or_an_interruption() {
    let plain = ReadError::new("keychain", "rc 1");
    assert_eq!((plain.is_locked(), plain.interruption()), (false, None));
    let locked = ReadError::locked("keychain", "rc 36: the keychain is locked");
    assert_eq!((locked.is_locked(), locked.interruption()), (true, None));
    let cut = ReadError::interrupted("keychain", 15);
    assert_eq!((cut.is_locked(), cut.interruption()), (false, Some(15)));
    assert_eq!(cut.to_string(), "keychain is unreadable: interrupted by signal 15");
}
```

  In `keychain.rs`:

```rust
#[test]
fn a_fake_secret_read_honours_the_interruptible_scope_as_the_security_driver_does() {
    let k = FakeKeychain::new();
    k.put("svc", "acct", b"secret");
    let token = crate::cancel::Cancel::new();
    token.request(2);
    assert!(k.find("svc", "acct").is_present(), "no scope, no interruption");
    {
        let _scope = crate::cancel::interruptible(&token);
        assert!(matches!(k.find("svc", "acct"), Read::Unreadable(e) if e.interruption() == Some(2)));
        k.upsert("svc", "acct", b"next").unwrap();
    }
    assert_eq!(k.read_counts().unwrap().tally(), ReadTally { answered: 1, locked: 0 });
    k.set_locked(true);
    assert!(matches!(k.find("svc", "acct"), Read::Unreadable(e) if e.is_locked()));
}
```

- [ ] **Step 2: Run them to verify they fail.** `cargo test -p tagteam-provider --lib -- security:: cancel:: keychain:: read::` fails to compile:
  - `Runner::run` takes 4 arguments, but `Scripted` and the tests pass 5;
  - `RunResult::Interrupted`, `cancel::interruptible`, `cancel::scoped`, `cancel::uninterruptible`, `ReadError::interrupted`, `ReadError::locked`, `interruption` and `is_locked` are not found;
  - `ProcessRunner::bounded` is not found.

- [ ] **Step 3: Implement.**
  - **`cancel.rs`:**
    - `thread_local! { static SCOPE: RefCell<Option<Cancel>> = const { RefCell::new(None) }; }`.
    - `interruptible(cancel)` replaces the slot with `Some(cancel.clone())` (the same shared cell) and keeps what it replaced. `uninterruptible()` replaces it with `None`. Dropping the guard puts back what it replaced, so nested guards dropped in reverse order (as `let` bindings are) restore each scope.
    - `scoped()` clones the slot.
    - Doc: H4, §14.1. A Keychain read on this thread is a cancellation point only while a scope lives. A write never is.
  - **`read.rs`:** a private `cause: Cause` field, with `#[derive(Debug, Clone, Copy, PartialEq, Eq)] enum Cause { Other, Locked, Interrupted(i32) }`. Nothing outside `read.rs` builds `ReadError` with a struct literal (checked), so the new field breaks no caller.
  - **`security.rs`:**
    - The adapter: `impl ProcessRunner { fn bounded(program: &str, args: &[String], stdin: Option<&[u8]>, timeout: Duration, cancel: Option<&Cancel>, grace: Duration) -> RunResult }`.
      - It builds `SpawnSpec { program: program.into(), args: args.iter().map(OsString::from).collect(), ..SpawnSpec::default() }`. The environment is inherited and no cwd is set, as now.
      - It calls `process::bounded(&spec, stdin.map(<[u8]>::to_vec), Output::Pipe, timeout, cancel, grace)` and maps the result:
        - `Exited { code, stdout, stderr, .. }` → `RunResult::Exited { code: code.unwrap_or(-1), .. }` (today's mapping);
        - `TimedOut` → `TimedOut`;
        - `Interrupted(n)` → `Interrupted(n)`;
        - `SpawnFailed(e)` → `SpawnFailed(e)`.
    - `Runner::run` for `ProcessRunner` is `Self::bounded(.., cancel, DRAIN_GRACE)`. The `Runner` doc says `cancel` kills the group as a timeout does.
    - `SecurityCli::run`: `let scope = cancel::scoped(); self.runner.run(SECURITY, &args, stdin, TIMEOUT, scope.as_ref())`.
    - `unreadable`: `Interrupted(n)` → `ReadError::interrupted("keychain", n)`; `Exited { code: 36, .. }` → `ReadError::locked("keychain", "rc 36: …")`, with the same text as today.
    - `disambiguation_failed`: `Interrupted(n)` → `ReadError::interrupted("keychain", n)`, never `-g`'s stderr.
    - `describe`: `Interrupted(n)` → `(None, "interrupted by signal {n}")`. Only `failed` reaches it, which a write never does.
    - `Debug`: `Interrupted` prints its signal.
    - `find`'s counting needs no change. Only rc 36 and `TimedOut` count as locked, so an interrupted read, which falls through to `other`, counts as neither. `lock_state`'s `_ => Unknown` covers `Interrupted`.
    - Delete `Waitable`, `Waited`, `wait_for`, `reap`, `collect`, the old `run_bounded`, the local `DRAIN_GRACE` and the `drain` import.
    - Imports: production gains `crate::cancel::{self, Cancel}` and `crate::process::{self, Output, SpawnSpec, Captured, DRAIN_GRACE}`. Imports only the tests still use (`thread`, `Instant`) move into `mod tests`, so clippy sees none unused.
  - **`keychain.rs`:**
    - `locked_read()` uses `ReadError::locked`.
    - `FakeKeychain::find` first returns `Read::Unreadable(ReadError::interrupted("keychain", n))` when `crate::cancel::scoped()` holds signal `n`. `FileKeychain` is left as it is.
    - The `Keychain::find` doc says a read inside a set scope may answer an interruption.
  - **xtask `Rec`:** add `_cancel: Option<&tagteam_provider::Cancel>`.

- [ ] **Step 4: Run them to verify they pass.**
  - `cargo test -p tagteam-provider --lib -- security:: cancel:: keychain:: read:: process::` PASS, inside the sandbox.
  - `cargo test -p xtask --lib the_lock_state_is_asked_of_the_file_that_is_locked` PASS.
  - **Outside the sandbox, macOS only:**
    - `cargo test -p tagteam-provider --locked --features real_keychain --test real_keychain` (CI's macOS step);
    - `cargo test -p tagteam-engine --features real_keychain --test run_real_keychain`.

    Both create a throwaway keychain and drive `/usr/bin/security` through the new adapter.
  - Then `cargo test -p tagteam-provider`, plus `cargo test -p tagteam-cc` and `cargo test -p tagteam-engine --features test-hooks`: those suites see the fakes' new `locked` marker. Text and counts are unchanged, so nothing should move.

- [ ] **Step 5: Commit**
  - Files: `crates/tagteam-provider/src/{security.rs,cancel.rs,read.rs,keychain.rs}`, `crates/xtask/src/compat/keychain.rs`, and any `Runner` call site fixed per **Files**.
  - Message: "Run security through the shared runner and let a scope interrupt its reads".

---

### Task 5: Keychain read retries and the interruptible scope in `live.rs` and `collect.rs` (H4)

**Depends on:** 4

**Files:**
- Modify: `crates/tagteam-cc/src/live.rs`: `retrying` (577-589) and `read_credential` (591-608). Imports gain `tagteam_provider::cancel` and `ReadError`.
- Modify: `crates/tagteam-engine/src/collect.rs`:
  - `stored_token` (863-882; the vault read at 866-870; the gate at 881);
  - `live_bytes` (its doc 917-936; the live read at 956; the result match 959-966).
  - Imports gain `tagteam_provider::cancel`.
- Test: `crates/tagteam-cc/tests/live_store.rs`: new tests and a `Flaky` double.
- Test: `crates/tagteam-engine/tests/collect.rs`, in `mod hooks` (1304-): one new test; `ReadTally` is added to the `tagteam_provider` import.
- Test: `crates/tagteam-engine/tests/collect_active.rs`, in `mod hooks` (613-): two new tests; `rescue_files` is added to the `common` import and `ReadTally` to the `tagteam_provider` import.

**Interfaces:**
- Consumes: Task 4's `cancel::{interruptible, scoped}`, `ReadError::{interrupted, interruption, is_locked}`, and `FakeKeychain` honouring the scope. The scope is what makes the engine tests below see reads that are really interrupted.
- Produces: behaviour only.

**Readings this task commits to:**
- **How collect reports an interrupted fetch today.** `Stop::Interrupted(signal)` (`collect.rs:612-616`) records nothing: `record_to_store` returns `EngineError::Interrupted` (`:1259-1262`), and `record` gives the unsent slot back best effort (`:1163-1176`). `collect_listed` and `collect_scheduled` then return `Err(EngineError::Interrupted)` from `check_cancel` (`:253`, `:282`, `:287`). An interrupted Keychain read maps onto exactly that `Stop`, the same as `interruption()` (`:682-687`).
- **An interrupted live read is never covered by the credentials file.** Today `read_credential` turns any unreadable Keychain read into `Degraded` from the file (`live.rs:603-606`), which `live_bytes` records as a `keychain-unavailable` failure (`collect.rs:960-962`). That would break "records nothing". The spec does not name this site; it follows from H4's rule.
- **Gate and §7.5 stay outside the scope.** The scope wraps only the two reads. `self.gate(&bytes)` (`:881`), `guard_or_refuse`, both identity checks, and `refresh_live`'s call to `refresh_active` run outside it, so their reads (the gate's compare-and-swap read at `refresh.rs:627`, `Vault::store`'s read and read-back, §7.5's live reads) are never interrupted. The re-read in `live_bytes` after §7.5 returns is in the scope: the critical span ended with the successor persisted.

- [ ] **Step 1: Write the failing tests.**

  In `crates/tagteam-cc/tests/live_store.rs` (imports gain `tagteam_provider::{Cancel, ReadError, cancel}`):

```rust
/// Secret reads that fail as a timeout does (not locked) `fails` times, then answer `bytes`;
/// the first read sets `token`, when there is one. Only `find` is ever called.
struct Flaky {
    fails: usize,
    bytes: Vec<u8>,
    token: Option<Cancel>,
    finds: AtomicUsize,
}

impl Flaky {
    fn new(fails: usize, bytes: &[u8], token: Option<Cancel>) -> Self {
        Self { fails, bytes: bytes.to_vec(), token, finds: AtomicUsize::new(0) }
    }
    fn finds(&self) -> usize {
        self.finds.load(Ordering::SeqCst)
    }
}

impl Keychain for Flaky {
    fn find(&self, _s: &str, _a: &str) -> Read<Vec<u8>> {
        let n = self.finds.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            if let Some(t) = &self.token {
                t.request(15);
            }
        }
        if n < self.fails {
            Read::Unreadable(ReadError::new("keychain", "security did not finish in time"))
        } else {
            Read::Present(self.bytes.clone())
        }
    }
    fn exists(&self, _s: &str, _a: &str) -> Read<()> { unreachable!() }
    fn upsert(&self, _s: &str, _a: &str, _d: &[u8]) -> Result<(), KeychainError> { unreachable!() }
    fn delete(&self, _s: &str, _a: &str) -> Result<(), KeychainError> { unreachable!() }
    fn lock_state(&self) -> LockState { unreachable!() }
    fn unlock(&self) -> bool { unreachable!() }
}

#[test]
fn a_locked_keychain_read_is_not_retried() {
    // H4, A.3: rc 36 will not change within 600 ms; the daemon's KeychainBackoff waits instead.
    let f = fx();
    f.kc.set_locked(true);
    let r = store(&f, Platform::MacOs).read_credential(&f.env, &f.paths);
    assert!(matches!(r, Read::Unreadable(e) if e.is_locked()));
    assert_eq!(f.kc.read_counts().unwrap().tally().locked, 1, "one read, no retry");
}

#[test]
fn any_other_unreadable_read_is_still_retried_twice() {
    let f = fx();
    let kc = Arc::new(Flaky::new(2, br#"{"claudeAiOauth":{}}"#, None));
    let s = LiveStore::new(kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO);
    assert!(s.read_credential(&f.env, &f.paths).is_present());
    assert_eq!(kc.finds(), 3);
}

#[test]
fn a_token_set_inside_the_scope_ends_the_retries_and_no_file_covers_the_read() {
    // H4: inside usage collection's scope, a set token stops the read between attempts, and an
    // interrupted read is never `Degraded` from the credentials file.
    let f = fx();
    fs::write(&f.paths.credentials_file, br#"{"claudeAiOauth":{"refreshToken":"file"}}"#).unwrap();
    let token = Cancel::new();
    let kc = Arc::new(Flaky::new(3, b"unused", Some(token.clone())));
    let s = LiveStore::new(kc.clone(), Platform::MacOs).with_retry_delay(Duration::ZERO);
    let _scope = cancel::interruptible(&token);
    let r = s.read_credential(&f.env, &f.paths);
    assert!(matches!(&r, Read::Unreadable(e) if e.interruption() == Some(15)), "{r:?}");
    assert_eq!(kc.finds(), 1);
}
```

  In `crates/tagteam-engine/tests/collect.rs`, `mod hooks` (Review Focus 5, first half):

```rust
#[test]
fn a_vault_read_the_signal_interrupts_records_nothing_and_is_no_usage_failure() {
    // Review Focus 5, H4, §14.1: the stored token's vault read is a cancellation point. An
    // interrupted read is not `keychain-unavailable`: nothing is recorded, the slot goes back.
    let fx = Fx::new();
    let a = two_accounts(&fx);
    fx.script_usage(200, usage_fixture());
    let reads = fx.kc.read_counts().unwrap().tally();
    signal_at(&fx, "usage-reserved");

    assert_interrupted(collect_result(&fx, &a));

    assert_eq!(
        fx.kc.read_counts().unwrap().tally().since(reads),
        ReadTally::default(),
        "the vault read was interrupted: never answered, never counted locked"
    );
    assert_eq!(fx.usage_state(&a), None, "no failure, no backoff: nothing is recorded");
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0, "the reserved slot went back");
}
```

  In `crates/tagteam-engine/tests/collect_active.rs`, `mod hooks` (Review Focus 5, both halves):

```rust
#[test]
fn a_live_read_the_signal_interrupts_records_nothing_and_no_file_covers_it() {
    // Review Focus 5: the live credential's read is a cancellation point; the credentials file,
    // which would make it a `Degraded` read and a `keychain-unavailable` failure, is not read.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    fs::write(fx.paths().credentials_file, credential("a@x.co", "rt-a")).unwrap();
    fx.script_usage(200, usage_fixture());
    let reads = fx.kc.read_counts().unwrap().tally();
    signal_at(&fx.engine, "usage-live-identity-read");

    assert_interrupted(&fx.engine, &a);

    assert_eq!(fx.kc.read_counts().unwrap().tally().since(reads).answered, 0);
    assert_eq!(fx.usage_state(&a), None, "nothing is recorded");
    assert!(fx.http.requests().is_empty());
    assert_eq!(usage_requests(&fx), 0, "the slot went back");
}

#[test]
fn a_signal_inside_active_token_refresh_never_interrupts_its_keychain_reads() {
    // Review Focus 5, §14.1: §7.5 is a critical span. Its reads run outside the scope; only
    // the live read after it is a cancellation point.
    let fx = Fx::new();
    let a = fx.add("a@x.co", "rt-a");
    expire_live(&fx);
    fx.script_refresh(Some("rt-a2"));
    fx.script_usage(200, usage_fixture());
    signal_at(&fx.engine, "active-after-response");

    assert_interrupted(&fx.engine, &a);

    assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-a2"), "published");
    assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"), "stored");
    assert_eq!(rescue_files(&fx), 0, "never diverted to rescue/");
    assert_eq!(quarantine_of(&fx, &a), (None, None));
    assert_eq!(methods(&fx), [Method::Post], "no usage request");
    assert_eq!(fx.usage_state(&a), None);
    assert_eq!(usage_requests(&fx), 0);
}
```

  The gate's half is pinned by the existing `a_gate_refresh_in_flight_when_the_signal_lands_still_persists_its_successor` (`collect.rs` hooks, ~:1903, signal at `gate-after-response`). Since Task 4's `FakeKeychain` honours the scope, a scope that leaked into the gate would interrupt `persist_successor`'s compare-and-swap read. The successor would then go to `rescue/`, and that test's `vault_refresh_token == "rt-a2"` would fail.

- [ ] **Step 2: Run them to verify they fail.**
  - `cargo test -p tagteam-cc --test live_store`:
    - `a_locked_keychain_read_is_not_retried` fails (locked is 3);
    - `a_token_set_inside_the_scope_…` fails: three reads, then a `Present` that the file made `Degraded`;
    - `any_other_unreadable_read_is_still_retried_twice` passes already, and pins what stays.
  - `cargo test -p tagteam-engine --features test-hooks --test collect --test collect_active`:
    - the two "interrupts" tests fail on the tally (one answered read: no scope yet);
    - `a_signal_inside_active_token_refresh_…` passes already, and pins what must stay.

- [ ] **Step 3: Implement.**
  - **`live.rs` `retrying`** retries only `Read::Unreadable(e)` with `!e.is_locked() && e.interruption().is_none()`. After each `retry_delay` sleep, when `cancel::scoped()` holds signal `n`, it returns `Read::Unreadable(ReadError::interrupted("keychain", n))` without reading again. Doc: A.3, plus H4's two rules.
  - **`read_credential`**: an `Unreadable(e)` with `e.interruption().is_some()` is returned before the file is consulted.
  - **`collect.rs` `stored_token`**:
    ```rust
    let read = {
        // H4, §14.1: a cancellation point. The gate below is not inside it.
        let _scope = cancel::interruptible(self.engine.cancel());
        self.engine.vault.read(&self.row.id)
    };
    ```
    Its `Unreadable(e)` arm becomes `Err(e.interruption().map_or_else(|| failed("keychain-unavailable"), Stop::Interrupted))`.
  - **`live_bytes`**: the same scope, around `self.p.read_live_auth(&self.engine.env)` only, and the same mapping for `Unreadable`. Add one line to its doc: the read is a cancellation point (H4), the guard and §7.5 are not.

- [ ] **Step 4: Run them to verify they pass.**
  - The two commands from Step 2, inside the sandbox.
  - Then `cargo test -p tagteam-cc`, `cargo test -p tagteam-engine --features test-hooks` and `cargo test -p tagteam-daemon`: rc 36 is now unretried everywhere `retrying` runs.
  - A search of `tagteam-cc`, `tagteam-engine` and `tagteam-daemon` tests found none that counts three reads of an rc-36 item. If one fails, it pinned the old retry, and it changes to one read.

- [ ] **Step 5: Commit**
  - Files: `crates/tagteam-cc/src/live.rs`, `crates/tagteam-engine/src/collect.rs`, `crates/tagteam-cc/tests/live_store.rs`, `crates/tagteam-engine/tests/collect.rs`, `crates/tagteam-engine/tests/collect_active.rs`.
  - Message: "Make usage collection's Keychain reads cancellation points and stop retrying a locked read".

---

### Task 6: `tagteam run` waits on `ExitWatch` and ticks at 250 ms (H9)

**Depends on:** 3

**Files:**
- Modify: `crates/tagteam/src/run.rs`:
  - `POLL` (22-23) becomes `TICK`, and the `RECORD_POLL` doc (25-27) follows;
  - `run_session`'s first look (102);
  - `wait_polling`'s doc (260-269) and its sleep (345);
  - `beside` and `StopOnDrop` (357-393);
  - `forward` (441-457);
  - `wait` (459-475);
  - `mod tests` (519-622).
- Modify: `crates/tagteam/tests/run_tokens.rs:251`: a comment only ("every 50 ms" → "every 250 ms").
- Test: `crates/tagteam/src/run.rs` (`mod tests`).
- Not touched: `crates/tagteam/src/tui_host.rs`. `run::wait`'s signature and contract stay (`tui_host.rs:106`).

**Interfaces:**
- Consumes: Task 3's `tagteam_provider::process::{ExitWatch, Watched}`.
- Produces:
  - `pub(crate) fn wait(child: &mut Child, cancel: &Cancel) -> (io::Result<ExitStatus>, Option<i32>)`: unchanged signature;
  - private `fn wait_every(child: &mut Child, cancel: &Cancel, tick: Duration, send: &mut dyn FnMut(u32, i32)) -> (io::Result<ExitStatus>, Option<i32>)`;
  - private `fn kill_child(pid: u32, signal: i32)`;
  - private `fn forward(pid: u32, signal: Option<i32>, first: bool, send: &mut dyn FnMut(u32, i32)) -> Option<i32>`.

- [ ] **Step 1: Write the failing tests** in `run.rs`'s `mod tests`. These existing tests must still pass:
  - the unit tests here: `the_first_look_after_the_spawn_…`, `an_exec_that_finds_no_command_…`, and the four `beside` tests;
  - `tests/run_cli.rs` (40), `tests/run_tokens.rs` (11) and `tests/tui_host.rs` (23), all of which passed inside the sandbox at 5c9a114. `tui_host.rs`'s `run_starts_tagteam_run_on_the_account_and_a_sigterm_is_forwarded_and_kept` and `a_sigint_during_the_session_is_the_session_s_and_a_sighup_is_forwarded` drive `run::wait`. Their 300 ms gap between signals outlasts a 250 ms tick, and a late tick still leaves the same recorded signal.
  - `tests/tui_pty.rs`'s `under_r_…` tests.

  Helpers (test code):

```rust
/// `pid`, a child of this process, is not reaped yet: waitid answers for it (running or a
/// zombie), and fails with ECHILD once it is reaped.
fn unreaped(pid: u32) -> bool {
    // SAFETY: a zeroed siginfo_t that waitid fills in; WNOHANG|WNOWAIT reap nothing.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) == 0
    }
}

/// `f`'s result from a thread of its own; a panic once `limit` has passed, so a wait that only
/// a tick would end fails instead of hanging.
fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit).expect("it did not return in time")
}
```

  Tests (Review Focus 3 is the first two):

```rust
#[test]
fn a_signal_pending_as_the_child_exits_is_forwarded_while_it_is_unreaped_never_after() {
    // Review Focus 3, H9: the child has exited, unreaped, when the wait first reads the token.
    let cancel = Cancel::new();
    let token = cancel.clone();
    let ((status, last), sends, pid) = within(Duration::from_secs(10), move || {
        let mut child = std::process::Command::new("/bin/sh").args(["-c", "exit 3"]).spawn().unwrap();
        let pid = child.id();
        assert_eq!(ExitWatch::start(pid).wait(Duration::from_secs(5)), Watched::Exited);
        token.request(libc::SIGTERM);
        let mut sends = Vec::new();
        let waited = wait_every(&mut child, &token, Duration::from_secs(3600), &mut |p, s| {
            sends.push((s, unreaped(p)))
        });
        (waited, sends, pid)
    });
    assert_eq!(status.unwrap().code(), Some(3));
    assert_eq!(last, Some(libc::SIGTERM));
    assert_eq!(sends, [(libc::SIGTERM, true)], "sent to the zombie, before the reap");
    assert!(!unreaped(pid), "reaped before the wait returned, so nothing can follow");
    assert_eq!(cancel.requested(), None, "taken once");
}

#[test]
fn a_signal_while_the_child_runs_reaches_it_within_a_tick_and_only_while_unreaped() {
    let d = tempfile::tempdir().unwrap();
    let started = d.path().join("started");
    let script = format!(
        "trap 'exit 7' TERM; : > '{}'; i=0; while [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done",
        started.display()
    );
    let cancel = Cancel::new();
    let (token, signaller) = (cancel.clone(), cancel.clone());
    let sender = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !started.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        signaller.request(libc::SIGTERM);
    });
    let ((status, last), sends) = within(Duration::from_secs(10), move || {
        let mut child = std::process::Command::new("/bin/sh").args(["-c", &script]).spawn().unwrap();
        let mut sends = Vec::new();
        let waited = wait_every(&mut child, &token, Duration::from_millis(20), &mut |p, s| {
            sends.push((s, unreaped(p)));
            kill_child(p, s); // deliver it, as `wait` does: the trap's exit 7 is the proof
        });
        (waited, sends)
    });
    sender.join().unwrap();
    assert_eq!(status.unwrap().code(), Some(7), "the forwarded SIGTERM ended it");
    assert_eq!(last, Some(libc::SIGTERM));
    assert_eq!(sends, [(libc::SIGTERM, true)]);
}

#[test]
fn the_child_s_exit_is_reported_without_waiting_for_a_tick() {
    // H14: an hour's tick; the exit wakes the wait.
    let ((status, last), sends) = within(Duration::from_secs(10), || {
        let mut child = std::process::Command::new("/bin/sh").args(["-c", "sleep 0.1; exit 5"]).spawn().unwrap();
        let mut sends = 0;
        let waited = wait_every(&mut child, &Cancel::new(), Duration::from_secs(3600), &mut |_, _| sends += 1);
        (waited, sends)
    });
    assert_eq!(status.unwrap().code(), Some(5));
    assert_eq!((last, sends), (None, 0));
}

#[test]
fn the_poller_is_woken_as_soon_as_the_wait_returns() {
    // H9: the poller sleeps a 250 ms tick, and `run` never waits it out at exit.
    let poller = |stop: &AtomicBool| {
        while !stop.load(Ordering::Relaxed) {
            thread::park_timeout(Duration::from_secs(20));
        }
    };
    let started = Instant::now();
    assert_eq!(beside(poller, || "exited", || 1), ("exited", 1));
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}
```

- [ ] **Step 2: Run them to verify they fail.** `cargo test -p tagteam --features test-support --lib run::tests` fails to compile: `wait_every` is not found. With a stub `wait_every`, `the_poller_is_woken_…` would fail on the elapsed bound after 20 s, because nothing unparks the poller.

- [ ] **Step 3: Implement** in `run.rs`.
  - **`TICK`:** `const TICK: Duration = Duration::from_millis(250);` replaces `POLL`. Doc: how often `run` forwards what the token recorded and checks the session record (H9); the child's exit wakes the wait at once. `RECORD_POLL` (2 s) is unchanged, and its doc says "on every `TICK`".
  - **`wait`:** `wait(child, cancel)` = `wait_every(child, cancel, TICK, &mut kill_child)`.
  - **`wait_every`**:
    - `let mut exit = ExitWatch::start(child.id());` Then loop on `exit.wait(tick)`:
      - `Watched::Failed(_)` → `(child.wait(), last)`: wait it out with no more forwarding, as today's failed `try_wait` did;
      - otherwise → `last = forward(pid, cancel.take(), false, send).or(last)`;
      - then, if the watch said `Exited` → `(child.wait(), last)`.
    - This thread is the only one that reaps, and it sends nothing once it has, so `forward`'s rule that the pid names the child or its zombie (`run.rs:445-447`) holds. The doc says so, and keeps the TUI paragraph.
  - **`forward`** filters with `forwarded` and calls `send(pid, signal)`.
  - **`kill_child`** is today's `libc::kill` and WARN. Its `SAFETY` comment names `wait_every` as the only reaper.
  - **`run_session`'s first look** is `forward(pid, cancel.take(), true, &mut kill_child)`.
  - **The poller** sleeps with `thread::park_timeout(TICK)` instead of `thread::sleep(POLL)`, on its own thread as now. Its store writes stay off the wait thread (H9).
  - **`beside`:** `StopOnDrop` holds `stop: &AtomicBool` and `poller: Option<thread::Thread>` (from the spawned handle's `thread()`). On drop it sets the flag, then unparks the poller. The poller's park returns at once (an unpark before the park is kept), so `run` exits as soon as the child does, instead of up to 250 ms later.
  - Edit the comment at `tests/run_tokens.rs:251`.

- [ ] **Step 4: Run them to verify they pass.**
  - `cargo test -p tagteam --features test-support --lib run::tests` PASS.
  - Inside the sandbox: `cargo test -p tagteam --features test-support --test run_cli --test run_tokens --test tui_host` PASS (40 + 11 + 23).
  - **Outside the sandbox** (openpty): `cargo test -p tagteam --features test-support --test tui_pty` PASS. This covers `under_r_a_sigterm_to_the_tui_is_forwarded_and_both_exit`, which waits on `tagteam run` through `run::wait`.
  - No existing test changes.

- [ ] **Step 5: Commit**
  - Files: `crates/tagteam/src/run.rs`, `crates/tagteam/tests/run_tokens.rs`.
  - Message: "Wait for tagteam run's child on its exit and tick every 250 ms".

---

### Task 7: Credentials as `-w` text; argv as a logged last resort (H5.1, H5.2)

**Depends on:** 1, 4

**Files:**
- Modify: `crates/tagteam-provider/src/security.rs` (`Keychain::upsert`, :449-493 at 5c9a114 — Task 4 shifts the lines but leaves `upsert`'s body as it is; drop `escape_text`'s `expect(dead_code)` attribute)
- Test: `crates/tagteam-provider/src/security.rs` `mod tests` (new tests; `long_writes_fall_back_to_argv`, :1027-1046, is replaced; a `logged` helper)
- Test: `crates/tagteam-provider/tests/real_keychain.rs` (one test through `upsert`)

**Interfaces:**
- Consumes: `escape_text` (Task 1); `text_cases`, `full_line_text`, `W_SERVICE` (Task 1's test helpers); `SecurityCli::run(&self, args: Vec<String>, stdin: Option<&[u8]>) -> RunResult` and `tail()` as Task 4 leaves them; `LINE_LIMIT` (4032). `upsert` keeps Task 4's `let _write = cancel::uninterruptible();` as its first line, so a write and its verify read are never interrupted.
- Produces: nothing new; `upsert`'s signature is unchanged.

- [ ] **Step 1: Write the failing tests**

  In `security.rs` `mod tests`, using `Scripted`, `cli`, `ok` and `og_hex` as they are, plus a `logged(f: impl FnOnce()) -> Vec<String>` helper of `atomic.rs`'s shape (:462-490: `tracing_subscriber::fmt().with_writer(..).with_ansi(false).without_time().finish()` under `tracing::subscriber::with_default`). Several tests in this module reach the WARN's call site on threads with no subscriber, so the helper also keeps a second dispatcher alive for the process, as the engine's `capture_logs` does (`tagteam-engine/tests/common/mod.rs:1563-1564`, `ALWAYS_TWO`; `tests/capture_logs.rs` explains why); otherwise the call site's interest can be cached as "never".
  ```rust
  #[test]
  fn a_text_credential_is_written_as_escaped_w_text_through_interactive_mode() {
      let data = r#"{"k":"a\"b\\c é"}"#.as_bytes(); // not `br`: byte literals are ASCII-only
      let s = Scripted::default().then(ok(b"")).then(ok(data));
      cli(&s, Some("/tmp/t.keychain")).upsert("svc", "acct", data).unwrap();
      let calls = s.calls();
      assert_eq!(calls.len(), 2, "the write, then its verify read");
      assert_eq!(calls[0].1, vec!["-i".to_string()]);
      assert_eq!(
          calls[0].2.as_deref().unwrap(),
          [r#"add-generic-password -U -a "acct" -s "svc" -w "{\"k\":\"a\\\"b\\\\c é\"}" "/tmp/t.keychain""#, "\n"]
              .concat()
              .as_bytes()
      );
  }

  #[test]
  fn bytes_with_nul_cr_or_lf_or_that_are_not_utf8_keep_the_hex_line() {
      for data in [&b"a\0b"[..], b"a\rb", b"a\nb", &[0xff, 0xfe]] {
          let hex = hex::encode(data);
          let s = Scripted::default()
              .then(ok(b""))
              .then(ok(format!("{hex}\n").as_bytes()))
              .then(og_hex(&hex.to_uppercase()));
          cli(&s, None).upsert("svc", "acct", data).unwrap();
          assert_eq!(
              s.calls()[0].2.as_deref().unwrap(),
              format!("add-generic-password -U -a \"acct\" -s \"svc\" -X \"{hex}\"\n").as_bytes(),
              "{data:?}"
          );
      }
  }

  #[test]
  fn a_text_line_of_exactly_the_limit_stays_on_standard_input() {
      // The line, newline included, is LINE_LIMIT bytes; one more byte takes argv.
      let fixed = "add-generic-password -U -a \"acct\" -s \"svc\" -w \"\"\n".len();
      let at_limit = "x".repeat(LINE_LIMIT - fixed);
      let s = Scripted::default().then(ok(b"")).then(ok(at_limit.as_bytes()));
      cli(&s, None).upsert("svc", "acct", at_limit.as_bytes()).unwrap();
      assert_eq!(s.calls()[0].2.as_ref().unwrap().len(), LINE_LIMIT);
      let over = format!("{at_limit}x");
      let s = Scripted::default().then(ok(b"")).then(ok(over.as_bytes()));
      cli(&s, None).upsert("svc", "acct", over.as_bytes()).unwrap();
      assert!(s.calls()[0].2.is_none(), "argv");
  }

  #[test]
  fn an_over_long_text_credential_goes_on_argv_as_raw_w_text_with_a_warn() {
      let data = format!(r#"{{"k":"a\"b\\c","s":"SENTINEL","pad":"{}"}}"#, "x".repeat(4100));
      let s = Scripted::default().then(ok(b"")).then(ok(data.as_bytes()));
      let lines = logged(|| cli(&s, None).upsert("svc", "acct", data.as_bytes()).unwrap());
      let (_, args, stdin) = &s.calls()[0];
      assert!(stdin.is_none());
      // argv needs no quoting: the text goes unescaped, never through `escape_text`'s output.
      assert_eq!(args, &argv(&["add-generic-password", "-U", "-a", "acct", "-s", "svc", "-w", &data], None));
      let warns: Vec<&String> = lines.iter().filter(|l| l.contains("WARN")).collect();
      assert_eq!(warns.len(), 1, "{lines:#?}");
      assert!(warns[0].contains(&format!("service=svc account=acct bytes={}", data.len())), "{}", warns[0]);
      assert!(lines.iter().all(|l| !l.contains("SENTINEL")), "never the value");
  }

  #[test]
  fn an_over_long_binary_credential_goes_on_argv_as_hex_with_a_warn() {
      // Replaces `long_writes_fall_back_to_argv`: 2100 bytes of `x` now fit as text.
      let data = vec![0xff; 2100]; // 4200 hex digits: over the line limit
      let s = Scripted::default().then(ok(b"")).then(ok(&data));
      let lines = logged(|| cli(&s, None).upsert("svc", "acct", &data).unwrap());
      let (_, args, stdin) = &s.calls()[0];
      assert!(stdin.is_none());
      assert_eq!(args, &argv(&["add-generic-password", "-U", "-a", "acct", "-s", "svc", "-X", &hex::encode(&data)], None));
      assert_eq!(lines.iter().filter(|l| l.contains("WARN") && l.contains("bytes=2100")).count(), 1, "{lines:#?}");
  }
  ```
  In `tests/real_keychain.rs` (CI's macOS step runs it):
  ```rust
  #[test]
  fn upsert_writes_every_text_case_and_reads_it_back_byte_for_byte() {
      let kc = TempKeychain::new();
      let k = SecurityCli::with_runner(Box::new(ProcessRunner), Some(kc.0.clone()));
      let mut cases = text_cases();
      cases.push(("full-line", full_line_text(&kc.0)));
      cases.push(("over-the-limit", format!("{{\"pad\":\"{}\\\"\"}}", "y".repeat(5000))));
      for (account, text) in &cases {
          k.upsert(W_SERVICE, account, text.as_bytes()).unwrap(); // `upsert` verifies by reading back
          assert_eq!(k.find(W_SERVICE, account).present().unwrap(), text.as_bytes(), "{account}");
      }
  }
  ```

- [ ] **Step 2: Run them to verify they fail**

  `cargo test -p tagteam-provider --lib security::tests` → `a_text_credential_is_written_as_escaped_w_text_through_interactive_mode` fails (the line carries `-X "<hex>"`), `a_text_line_of_exactly_the_limit_stays_on_standard_input` fails (the hex line takes argv), `an_over_long_text_credential_goes_on_argv_as_raw_w_text_with_a_warn` fails on `-X` and on no WARN line, the binary argv test fails on no WARN line. The hex-line test passes already (it pins the behaviour kept).

- [ ] **Step 3: Implement**

  In `upsert`, after both `check_name` calls:
  - `let text = escape_text(data);` The value is `-w "<text>"` when `Some`, else `-X "<hex::encode(data)>"`; the line is `add-generic-password -U -a "<account>" -s "<service>" <value>{self.tail()}\n`, as today apart from the value.
  - `line.len() <= LINE_LIMIT` → `self.run(s(&["-i"]), Some(line.as_bytes()))`, as today.
  - Otherwise argv with the same encoding: `["add-generic-password", "-U", "-a", account, "-s", service, "-w", <the credential as &str, unescaped>]` when `text` is `Some` (the bytes are known UTF-8; never pass `escape_text`'s output on argv, whose quotes and backslashes would be stored literally), else `[…, "-X", &hex]`; `run` appends the test keychain file as now. Before running it, log exactly:
    `tracing::warn!(service = %service, account = %account, bytes = data.len(), "a Keychain value is too long for security -i, so it goes on security's command line");`
    The account here is the Keychain item's account attribute: tagteam's account ID (`<id>`, `<id>.prev`) for vault items, the OS user name for Claude Code's items; never an email.
  - No fallback from a too-long text line to the hex line: escaping adds at most one byte per byte and hex always adds exactly one, so the hex line is never shorter than the text line. The hex line is used only when `escape_text` is `None`.
  - The verify read after the write is unchanged.

- [ ] **Step 4: Run them to verify they pass**

  - `cargo test -p tagteam-provider --lib security::tests` → PASS. `long_writes_fall_back_to_argv` is deleted (replaced above); every other existing test passes unmodified. (These tests and this `upsert` compiled and passed on a prototype, clippy clean with and without `test-support`; the real-keychain test was not run there.)
  - Outside the sandbox, as in Task 1: `cargo test -p tagteam-provider --locked --features real_keychain --test real_keychain` → PASS, 7 tests (this exercises argv `-w` for the 5000-byte case and the existing `vec![b'x'; 5000]` case, and the hex line for `[0, 1, 2, 255]`).

- [ ] **Step 5: Commit**

  `crates/tagteam-provider/src/security.rs`, `crates/tagteam-provider/tests/real_keychain.rs` — "Write Keychain credentials as -w text and log the argv fallback"

---

### Task 8: Machine-shared keys out of the vault, rescues and `--full` exports (H5.3)

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-provider/src/provider.rs` (`Provider` trait, :534-867: `vault_form` after `fingerprint`, :571)
- Modify: `crates/tagteam-cc/src/shape.rs` (`pub fn vault_form` beside `compose`, :194-213; `export_credential`'s `full` branch, :279-296)
- Modify: `crates/tagteam-cc/src/provider.rs` (`impl Provider for ClaudeCode`: `vault_form` after `fingerprint`, :365-367)
- Modify: `crates/tagteam-fake/src/shape.rs` (`pub(crate) fn vault_form` beside `compose`, :109-126; `export_credential`, :130-141)
- Modify: `crates/tagteam-fake/src/provider.rs` (`impl Provider for FakeAgent`: `vault_form` after `fingerprint`, :367-369)
- Modify: `crates/tagteam-engine/src/vault.rs` (`Vault` doc, :125-126; `Vault::store`, :148-182)
- Modify: `crates/tagteam-engine/src/refresh.rs` (`persist_generation`'s store call, :348; `persist_received`'s landed check, :578; `Received`, :128-190: it keeps its provider)
- Modify: `crates/tagteam-engine/src/rescue.rs` (`write_rescue`, :80-120, takes the provider and writes the vault form)
- Modify: `crates/tagteam-engine/src/lifecycle.rs` (drop the `fp` closure, :740; the two store calls, :767 and :799)
- Modify: `crates/tagteam-engine/src/switch.rs` (:2271, the store call's **arguments only**, H1.2)
- Test: `crates/tagteam-cc/src/shape.rs` `mod tests` (:361-); `crates/tagteam-fake/src/shape.rs` `mod tests` (:159-); `crates/tagteam-core/src/classify.rs` `mod tests`; `crates/tagteam-engine/src/rescue.rs` `mod tests`
- Test: `crates/tagteam-engine/tests/vault.rs` (rewritten over `FakeAgent`), `tests/switch.rs`, `tests/export.rs`, `tests/gate_persist.rs`, `tests/common/mod.rs` (:737)
- Test (existing tests that pin the old behaviour, updated in Step 4): `crates/tagteam-cc/tests/transfer.rs`, `crates/tagteam-fake/tests/provider.rs`, `crates/tagteam-engine/tests/fake_agent.rs`, `crates/tagteam-engine/tests/provenance.rs`

**Interfaces:**
- Consumes: `shape::MACHINE_SHARED_KEYS` (CC), `shape::DEVICE` (FakeAgent).
- Produces:
  - `fn vault_form<'a>(&self, credential: &'a [u8]) -> Cow<'a, [u8]>` on `Provider`, default `Cow::Borrowed(credential)` (brief's fixed interface); CC's and FakeAgent's overrides.
  - `tagteam_cc::shape::vault_form(bytes: &[u8]) -> Cow<'_, [u8]>` (pub, like `compose`).
  - `Vault::store(&self, lock: &AccountLock, bytes: &[u8], p: &dyn Provider) -> Result<(), VaultError>` (was `fingerprint: &dyn Fn(&[u8]) -> Option<Fingerprint>`).
  - `Engine::write_rescue(&self, p: &dyn Provider, id: &AccountId, login_epoch: i64, predecessor_fp: &str, successor: &[u8], successor_fp: &Fingerprint) -> Result<PathBuf, EngineError>`; `Received<'e>` gains `provider: &'e dyn Provider`, so `Received::new` takes `p: &'e dyn Provider`. Task 9 builds on both.

**Decision: how `store` reaches the provider.** It takes `p: &dyn Provider`, and uses `p.fingerprint` and `p.vault_form`. Every caller already holds `p`, every write of a credential into the vault goes through the one method, and a caller cannot store without the form. Rejected: callers applying `vault_form` before `store` (four call sites and every future one must remember it; nothing enforces it), and a narrow `Lineage` trait with a generic `store` (keeps `tests/vault.rs`'s byte literals, but adds public API for one method). The cost is rewriting `tests/vault.rs` over `FakeAgent`.

- [ ] **Step 1: Write the failing tests (providers and exports)**

  `crates/tagteam-cc/src/shape.rs` `mod tests` (add `use std::borrow::Cow;`):
  ```rust
  #[test]
  fn the_vault_form_drops_exactly_the_machine_shared_keys_and_keeps_the_order() {
      let full = json!({"claudeAiOauth": {"refreshToken": "rt"}, "mcpOAuth": {"s": 1},
          "trustedDeviceToken": "d", "mcpOAuthClientConfig": {}, "designOauth": {"t": 1},
          "mcpXaaIdp": {}, "mcpXaaIdpConfig": {}, "pluginSecrets": {"p": "s"}, "futureKey": 1})
          .to_string().into_bytes();
      let kept = json!({"claudeAiOauth": {"refreshToken": "rt"}, "trustedDeviceToken": "d",
          "designOauth": {"t": 1}, "futureKey": 1}).to_string();
      assert_eq!(vault_form(&full).as_ref(), kept.as_bytes());
      assert_eq!(fingerprint(&vault_form(&full)), fingerprint(&full), "the lineage is unchanged");
  }

  #[test]
  fn the_vault_form_of_anything_else_is_the_bytes_themselves() {
      // No machine-shared key (byte-identical, spacing included), an API key, not JSON, not an object.
      for bytes in [&br#"{ "claudeAiOauth": {"refreshToken": "rt"} }"#[..], b" sk-ant-api03-k\n", b"garbage", b"[1]"] {
          assert!(matches!(vault_form(bytes), Cow::Borrowed(b) if b == bytes), "{bytes:?}");
      }
  }

  #[test]
  fn a_full_export_keeps_every_account_scoped_key_and_no_machine_shared_one() {
      let stored = json!({"claudeAiOauth": {"refreshToken": "rt"}, "mcpOAuth": {"s": 1},
          "trustedDeviceToken": "d", "pluginSecrets": {"p": "s"}, "designOauth": {"t": 1}});
      assert_eq!(
          export_credential(stored.to_string().as_bytes(), true).unwrap(),
          json!({"claudeAiOauth": {"refreshToken": "rt"}, "trustedDeviceToken": "d", "designOauth": {"t": 1}})
      );
      assert_eq!(export_credential(b"sk-ant-api03-k", true).unwrap(), json!("sk-ant-api03-k"));
  }
  ```
  `crates/tagteam-fake/src/shape.rs` `mod tests`:
  ```rust
  #[test]
  fn the_vault_form_drops_only_the_device_key() {
      let full = serde_json::to_vec(&crate::credential_json("t", Some("r"), Some(9))).unwrap();
      assert_eq!(vault_form(&full).as_ref(), json!({"fa": {"token": "t", "renew": "r", "expires": 9}}).to_string().as_bytes());
      let slim = json!({"fa": {"token": "t"}}).to_string().into_bytes();
      assert!(matches!(vault_form(&slim), std::borrow::Cow::Borrowed(_)));
  }

  #[test]
  fn a_full_export_drops_the_device_key_too() {
      let full = serde_json::to_vec(&crate::credential_json("t", Some("r"), None)).unwrap();
      assert_eq!(export_credential(&full, true).unwrap(), json!({"fa": {"token": "t", "renew": "r"}}));
  }
  ```

- [ ] **Step 2: Run them to verify they fail**

  `cargo test -p tagteam-cc --lib shape::tests` and `cargo test -p tagteam-fake --lib shape::tests` → compile errors: cannot find function `vault_form`.

- [ ] **Step 3: Implement (providers and exports)**

  - The trait method with its default, documented: "The credential as tagteam keeps it, in the vault and `rescue/` (H5.3): what is not the account's to keep is dropped. The default keeps every byte."
  - CC `shape::vault_form`: parse; anything that is not a JSON object → `Cow::Borrowed(bytes)`; an object holding none of `MACHINE_SHARED_KEYS` at the top level → `Cow::Borrowed(bytes)` (never re-serialized, so bytes that were already the form stay byte-identical); otherwise `shift_remove` each of the five and `Cow::Owned(serde_json::to_vec(..))` (`preserve_order` keeps the rest in order). `ClaudeCode::vault_form` calls it.
  - FakeAgent `shape::vault_form`: the same with `DEVICE` alone; `FakeAgent::vault_form` calls it.
  - Both `export_credential`s: `full` drops the provider's machine-shared keys too (CC: the five, before returning the object; FakeAgent: `shift_remove(DEVICE)` whatever `full` says). Update both doc comments: `full` keeps every account-scoped key, `trustedDeviceToken` included, and no machine-shared one (§13.3 as amended by H13).

- [ ] **Step 4: Run them to verify they pass; update the export tests that pin the old payload**

  `cargo test -p tagteam-cc -p tagteam-fake` → PASS once these are changed:
  - `crates/tagteam-cc/tests/transfer.rs` `a_slim_export_keeps_only_claude_ai_oauth_and_a_full_one_keeps_everything` → rename `..._and_a_full_one_keeps_every_account_scoped_key`; assert `full == json!({"claudeAiOauth": stored("rt-1")["claudeAiOauth"], "trustedDeviceToken": "device-bound", "designOauth": {"t": 1}})`.
  - `crates/tagteam-fake/tests/provider.rs` `its_export_payload_is_its_own_shape_with_no_claude_code_field` (:1128): `assert_eq!(full, slim)` (FakeAgent's full payload minus `device` is its slim one).

- [ ] **Step 5: Write the failing tests (engine)**

  `crates/tagteam-engine/tests/vault.rs`: replace the `fp` test lineage with `FakeAgent` (engine already dev-depends on `tagteam-fake`):
  ```rust
  /// A FakeAgent credential of generation `renew`, told apart within it by `token`. It has no
  /// machine-shared key, so its vault form is itself.
  fn cred(renew: &str, token: &str) -> Vec<u8> {
      serde_json::to_vec(&json!({"fa": {"token": token, "renew": renew}})).unwrap()
  }
  ```
  Every existing test keeps its assertions with `b"gen1#a"` → `cred("gen1", "a")`, `b"gen1#b"` → `cred("gen1", "b")`, `b"gen2"` → `cred("gen2", "t")`, `b"g"` → `cred("g", "t")`, and `&fp` → `&agent` (`let agent = FakeAgent::new();`). New:
  ```rust
  #[test]
  fn store_writes_and_verifies_the_vault_form() {
      // H5.3. Were the read-back compared with the bytes given, this store would fail `Verify`.
      let d = tempfile::tempdir().unwrap();
      let env = Env::for_test(d.path());
      let agent = FakeAgent::new();
      for v in backends(&env, Arc::new(FakeKeychain::new())) {
          let id = AccountId::from_string("f");
          let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
          let full = serde_json::to_vec(&tagteam_fake::credential_json("t", Some("gen1"), None)).unwrap();
          v.store(&lock, &full, &agent).unwrap();
          assert_eq!(v.read(&id).present().unwrap(), cred("gen1", "t"));
      }
  }

  #[test]
  fn an_item_stored_before_this_branch_is_read_as_it_is_and_leaves_for_prev_in_vault_form() {
      // H5.3: nothing migrates an old item; it keeps its bytes until its next store.
      let d = tempfile::tempdir().unwrap();
      let env = Env::for_test(d.path());
      let kc = Arc::new(FakeKeychain::new());
      let v = Vault::new(Box::new(KeychainVault::new(kc.clone())));
      let agent = FakeAgent::new();
      let id = AccountId::from_string("old");
      let lock = AccountLock::acquire(&env, &id, AccountLock::WAIT).unwrap();
      let old = serde_json::to_vec(&tagteam_fake::credential_json("t", Some("gen1"), None)).unwrap();
      kc.put(SERVICE, id.as_str(), &old);
      assert_eq!(v.read(&id).present().unwrap(), old);
      v.store(&lock, &cred("gen2", "t"), &agent).unwrap();
      assert_eq!(v.read_prev(&id).present().unwrap(), cred("gen1", "t"));
  }
  ```
  `crates/tagteam-core/src/classify.rs` `mod tests`:
  ```rust
  #[test]
  fn a_live_credential_that_differs_from_the_vault_only_by_machine_shared_keys_keeps_its_action() {
      // H5.3: the vault drops machine-shared keys, so `bytes_equal_vault` turns false whenever the
      // live entry carries them. A token-bearing credential stays `Ours` through its fingerprint;
      // a tokenless one turns `Wiped`, with the same action.
      let token_bearing = OutgoingFacts { fp_equal_vault: true, ..facts() };
      assert_eq!(decide_outgoing(&token_bearing), (OutgoingClass::Ours, OutgoingAction::Nothing));
      let tokenless_before = OutgoingFacts { bytes_equal_vault: true, tokenless: true, ..facts() };
      let tokenless_after = OutgoingFacts { tokenless: true, ..facts() };
      assert_eq!(decide_outgoing(&tokenless_before), (OutgoingClass::Ours, OutgoingAction::Nothing));
      assert_eq!(decide_outgoing(&tokenless_after), (OutgoingClass::Wiped, OutgoingAction::Nothing));
  }
  ```
  `crates/tagteam-engine/tests/switch.rs` (uses `capture_logs`, `credential`, `displaced_files`, `to`, `switch`):
  ```rust
  #[test]
  fn an_outgoing_live_entry_that_carries_machine_shared_keys_the_vault_lacks_is_ours() {
      let fx = Fx::new();
      let a = fx.add("a@x.co", "rt-a");
      let b = fx.add("b@x.co", "rt-b"); // live, `mcpOAuth` included; the vault copy has none
      let stored = fx.vault_bytes(&b).unwrap();
      assert!(serde_json::from_slice::<serde_json::Value>(&stored).unwrap().get("mcpOAuth").is_none());
      let (out, logs) = capture_logs(|| switch(&fx, to(&a), false).unwrap());
      assert!(out.switched);
      assert!(
          !logs.iter().any(|l| l.contains("stored a credential in the vault") && l.contains(b.as_str())),
          "classified Ours, never captured: {logs:#?}"
      );
      assert_eq!(fx.vault_bytes(&b).unwrap(), stored);
      assert_eq!(displaced_files(&fx), 0);
  }

  #[test]
  fn a_vault_item_stored_before_this_branch_still_classifies_and_switches() {
      // Review Focus line 2: full bytes, machine-shared keys included, byte-identical to live.
      let fx = Fx::new();
      let a = fx.add("a@x.co", "rt-a");
      let b = fx.add("b@x.co", "rt-b");
      let old = credential("b@x.co", "rt-b");
      fx.put_vault(&b, &old);
      switch(&fx, to(&a), false).unwrap();
      assert_eq!(fx.vault_bytes(&b).unwrap(), old, "Ours: nothing captured, nothing migrated");
      assert_eq!(displaced_files(&fx), 0);
      switch(&fx, to(&b), false).unwrap();
      assert_eq!(fx.live_refresh_token().as_deref(), Some("rt-b"));
      assert_eq!(fx.live_credential().unwrap()["mcpOAuth"], json!({"srv": {"token": "machine-shared"}}));
  }
  ```
  `crates/tagteam-engine/tests/export.rs` (uses `running`, `two_accounts`, `sources`, `envelope`):
  ```rust
  #[test]
  fn a_full_export_carries_no_machine_shared_key_from_the_live_login_or_a_session_profile() {
      let full = ExportRequest { full: true, ..ExportRequest::default() };
      let keys = |c: &Value| c.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
      // Live-sourced: Claude Code rotated b's token since the capture (`transfer::newest_live`).
      let fx = Fx::new();
      two_accounts(&fx);
      fx.rotate_live("rt-b2");
      let r = fx.engine.export(&full).unwrap();
      assert_eq!(sources(&r)[1], (2, Source::Live, true));
      assert_eq!(keys(&envelope(&r)[1]["credential"]), ["claudeAiOauth"]);
      assert_eq!(envelope(&r)[1]["credential"]["claudeAiOauth"]["refreshToken"], "rt-b2");
      // Profile-sourced: a running session's profile rotated a's token.
      let fx = Fx::new();
      let a = two_accounts(&fx);
      let _session = running(&fx, &a, "rt-a", "rt-a2");
      let r = fx.engine.export(&full).unwrap();
      assert_eq!(sources(&r)[0], (1, Source::Profile, true));
      assert_eq!(keys(&envelope(&r)[0]["credential"]), ["claudeAiOauth"]);
  }
  ```
  `crates/tagteam-engine/tests/gate_persist.rs` (top level, not `hooked`; add `use std::sync::atomic::{AtomicBool, Ordering};`, `tagteam_provider::{Clock, ReadError}` (`Clock` for `fx.clock.now_ms()`), `common::rescue_files`):
  ```rust
  /// A vault whose first read of `target` after writing it fails: a read-back that could not
  /// confirm a write that landed.
  struct ReadBackFailsOnce { inner: KeychainVault, target: String, armed: AtomicBool }
  // read: `key == target && armed.swap(false)` → `Read::Unreadable(ReadError::new("vault", "the read-back failed"))`,
  // else `inner.read`; write: `inner.write`, then arm when `key == target`; delete: `inner.delete`.

  #[test]
  fn a_landed_write_whose_read_back_failed_loses_nothing_when_the_successor_has_machine_shared_keys() {
      // H5.3: `persist_received` compares the vault form of what it received. The vault still
      // holds a pre-branch item, so the successor (built from it) carries `mcpOAuth`.
      let fx = Fx::new();
      let a = due(&fx);
      let mut old = Fx::credential_json("a@x.co", "rt-a");
      old["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms() + 60_000);
      fx.put_vault(&a, old.to_string().as_bytes());
      let snapshot = fx.vault_bytes(&a).unwrap();
      fx.script_refresh(Some("rt-a2"));
      let engine = fx.engine_with_vault(Vault::new(Box::new(ReadBackFailsOnce {
          inner: KeychainVault::new(fx.kc.clone()), target: a.to_string(), armed: AtomicBool::new(false),
      })));
      let out = engine.refresh_stored(fx.cc.as_ref(), &a, &snapshot).unwrap();
      assert!(matches!(out, GateOutcome::Refreshed(_)), "{out:?}");
      let stored: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
      assert_eq!(stored["claudeAiOauth"]["refreshToken"], "rt-a2");
      assert!(stored.get("mcpOAuth").is_none(), "the vault form");
      assert_eq!(rescue_files(&fx), 0, "nothing was kept outside the vault");
  }
  ```
  `crates/tagteam-engine/src/rescue.rs` `mod tests`:
  ```rust
  #[test]
  fn a_rescue_envelope_holds_the_vault_form() {
      let t = T::new();
      let id = AccountId::from_string("acct");
      let mut full: Value = serde_json::from_slice(&cred("rt-2", 9)).unwrap();
      full["mcpOAuth"] = json!({"srv": {"token": "machine-shared"}});
      let full = full.to_string().into_bytes();
      let fp = t.cc.fingerprint(&full).unwrap();
      let path = t.engine.write_rescue(t.cc.as_ref(), &id, 0, &t.fp(&cred("rt-1", 9)), &full, &fp).unwrap();
      let v: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
      assert_eq!(v["credential"].as_str().unwrap().as_bytes(), cred("rt-2", 9));
  }
  ```

- [ ] **Step 6: Run them to verify they fail**

  `cargo test -p tagteam-engine --features test-hooks --test vault --test switch --test export --test gate_persist` and `cargo test -p tagteam-engine --lib rescue::tests` → compile errors (`store` expects a closure, `write_rescue` takes five arguments). Once those compile in Step 7's first edit, `store_writes_and_verifies_the_vault_form`, the item-before-this-branch `.prev` test, the full-export test, the read-back test (outcome `Transient { kind: "vault-write", rescued: true }`) and the envelope test fail on their assertions. `cargo test -p tagteam-core --lib classify` passes already: it pins the table, which does not change. The two `switch.rs` tests pass before and after: they pin the outcome H5.3 says is unchanged.

- [ ] **Step 7: Implement (engine)**

  - `Vault::store(&self, lock, bytes, p)`: `let form = p.vault_form(bytes);` then everything as today on `&form`: the fingerprint is `p.fingerprint(&form)`, the `.prev` rotation compares `p.fingerprint(&old)`, the write and the read-back use `&form`. The `.prev` write stores `p.vault_form(&old)`: a pre-branch item's machine-shared keys leave the vault with its next store (H1.1 criterion 2; the fingerprint is unchanged, and every reader of `.prev` compares fingerprints). The doc comment: lineage and the stored form come from the caller's provider; the read-back compares the form written.
  - Callers: `refresh.rs:348` → `self.vault.store(lock, bytes, p)?`; `lifecycle.rs:767`/`:799` → `…, secret, p)` and delete the `fp` closure (:740); `switch.rs:2271` → `self.vault.store(lock, &bytes, p)?` (arguments only; record it in the MR as H1.2's second touch); `tests/common/mod.rs:737` → `.store(&lock, new_bytes, self.cc.as_ref())`.
  - `refresh.rs:578`: `Read::Present(b) if *b == *p.vault_form(&received.bytes)`.
  - `rescue.rs` `write_rescue` takes `p: &dyn Provider` first and serializes `p.vault_form(successor)` as the envelope's `credential` (`displaced/` is untouched: `keep_foreign` stays verbatim). Its doc says so. `Received` (`refresh.rs:128-190`) stores `provider: &'e dyn Provider` from `Received::new(engine, p: &'e dyn Provider, …)` and passes it in `keep`. Update the five `write_rescue` calls in `rescue.rs`'s tests to pass `t.cc.as_ref()` first.
  - `OutgoingFacts.bytes_equal_vault` (`switch.rs:2251`) and `recover.rs:390` stay as they are (H5.3).

- [ ] **Step 8: Run them to verify they pass; update the tests that pin vault bytes**

  `cargo test --workspace --locked --features tagteam/test-support` → PASS (the sandbox's usual pty and socket failures aside), after these existing tests change (found by running the whole suite against a prototype of this task, on which every new test above also passed):
  - `crates/tagteam-engine/tests/fake_agent.rs` `a_fake_agent_account_has_no_claude_code_shaped_field` (:116-119): the stored credential is `json!({"fa": credential_json(..)["fa"]})`; assert `stored.get("device").is_none()`.
  - `crates/tagteam-engine/tests/provenance.rs` `a_rotated_profile_is_captured_when_its_token_expires_earlier` (:125-126) and `a_rotated_profile_is_still_captured_after_the_data_directory_moved` (:192-193): compare `bytes` and `fx.vault_bytes(&a)` with `fx.cc.vault_form(&rotated).into_owned()`, not `rotated`.
  - `crates/tagteam-engine/tests/export.rs` `an_idle_account_is_exported_slim_from_the_vault_and_the_live_one_from_where_it_advances` (:92-95): the full export is `json!({"claudeAiOauth": Fx::credential_json("a@x.co", "rt-a")["claudeAiOauth"]})`.
  - `tests/vault.rs` and `tests/common/mod.rs:737` as in Steps 5 and 7.
  Also run, outside the sandbox, the ignored real-keychain provenance sequence, which carries MCP tokens through a session's vault: `cargo test -p tagteam-engine --locked --features real_keychain --test run_real_keychain -- --ignored` → PASS (its MCP token comes from the profile's own credential, §12.3 step 4, never the vault).

- [ ] **Step 8a: The direct branch compares the vault form (ruling R1)**

  `switch.rs:2043` displaces whatever live entry is not byte-identical to the target's vault bytes. With the vault form, `tagteam switch --force <the live account>` would save the live entry (machine-shared keys included) to `displaced/` and warn "the previous live credential was saved as displaced/…"; before, the bytes were identical and nothing was saved (verified on a prototype). Write the failing tests first, in `crates/tagteam-engine/tests/switch.rs`: `forcing_a_switch_to_the_live_account_displaces_nothing` — with `b` live and its live entry carrying machine-shared keys, `switch(&fx, to(&b), true)` succeeds and leaves `displaced_files(&fx) == 0` with no displaced warning; and `forcing_a_switch_to_the_live_account_with_a_pre_branch_vault_item_displaces_nothing` — the same, but `b`'s vault item is first overwritten with the live entry's full bytes (machine-shared keys included, as a vault item written before this branch holds them; write it through the backend, not `Vault::store`). Then change that one line to compare both operands in vault form: `.filter(|b| *p.vault_form(b) != *p.vault_form(&secret))`. This is H1.2's third touch of `switch.rs` (ruling R1); record it in the MR. Run `cargo test -p tagteam-engine --features test-hooks --test switch` → PASS (the new test and all existing ones).

- [ ] **Step 9: Commit**

  All files above — "Keep machine-shared keys out of the vault, rescue/ and full exports"

---

### Task 9: Write the rescue ahead of the vault; the landed rule (H6)

**Depends on:** 8

**Files:**
- Modify: `crates/tagteam-engine/src/refresh.rs` (`Received`, :128-190: an `ahead` field, the write-ahead in `new`, `keep` and `landed`; `persist_received`, :562-606: `landed()` on both paths where the vault holds the successor)
- Modify: `crates/tagteam-engine/src/rescue.rs` (`write_rescue`, :80-120, loses its INFO line, which moves to `Received::keep`; `settle_rescues`, :202-251, applies the landed rule first)
- Modify: `crates/tagteam-engine/src/active.rs` (`reconcile_active`, :492-607: the landed rule before the table; the in-step branch's own landed loop, :541-553, and the `.prev` branch's `retire` filter, :560-564, are subsumed)
- Modify: `crates/tagteam-engine/src/transfer.rs` (`successor`, :782-786, and `newest_live`, :788-811, skip a landed rescue)
- Test: `crates/tagteam-engine/src/rescue.rs` `mod tests`; `crates/tagteam-engine/src/transfer.rs` `mod tests` (:869-); `crates/tagteam-engine/tests/gate_persist.rs` (top level and `hooked`); `tests/active.rs`; `tests/export.rs`; `crates/tagteam/tests/gate_race.rs` (one real-kill test)

**Interfaces:**
- Consumes: Task 8's `Engine::write_rescue(p, id, login_epoch, predecessor_fp, successor, successor_fp)` (writes the vault form), `Received.provider`, `Provider::vault_form`; the hook points `gate-after-response`, `gate-before-vault-write`, `active-after-response`; `Engine::{on_point, fail_at}`; `TAGTEAM_TEST_CRASH_AT`.
- Produces: `fn landed(&mut self)` on `Received` (replaces `disarm`); `fn landed(rescue: &PendingRescue, vault: &Fingerprint) -> bool` (private, `transfer.rs`). No public API changes.

**The rules, as built.**
- *Write-ahead.* `Received::new`, when `foreign` is `None`, calls `engine.write_rescue(p, &row.id, row.login_epoch, predecessor_fp, &bytes, &fp)` before it returns, so the envelope is on disk before any hook point, the gate's vault re-read in `persist_successor`, or active refresh's foreign check; `Received::new` is the moment both callers know the successor is the account's (`foreign` is fixed there). `Ok(path)` → `ahead = Some(path)`, then DEBUG; `Err` → `ahead = None`, then WARN, and nothing else changes (best effort). A foreign successor writes nothing ahead and still goes to `displaced/` only.
- *Keep.* `keep()` for an own successor writes an envelope only when `ahead` is `None`; either way it then logs rescue.rs:115's INFO line ("kept a refreshed token in rescue/ until the vault takes it", `account`, `fp`), which moves here from `write_rescue` (keep first, then log, as now). So the envelope written ahead is the rescue whenever the vault fails, and no path (the vault failing, the vault having moved, `abandon`, `Drop`) writes a second copy.
- *Landed.* `landed()` disarms and, best effort, deletes `ahead`; a failed delete logs WARN and is harmless (the landed rule removes the file later). `persist_received` calls it where it calls `disarm()` today: after `persist_generation` succeeds, and when the vault write landed but recording it failed (:578). Remove `disarm`.
- *The landed rule:* a rescue whose own fingerprint, `p.fingerprint(&credential)`, is `Some` and equals the vault's current fingerprint.
  - `settle_rescues`: after the unreadable-file refusal and the vault read (`current`), a first pass deletes every landed entry (DEBUG on success; WARN on failure, and the entry is still skipped, never adopted); the adoption loop then runs over the rest, unchanged.
  - `reconcile_active`: once `vault` and `rescues` are read, delete (with `?`, as the function's other deletes) every rescue landed against the vault's fingerprint and drop it from `rescues`; remember whether any went. The in-step branch's own "landed everywhere" loop (:541-547) then never finds anything and goes; that branch returns `changed: <whether the first pass deleted any>`. The `.prev` branch's `retire` filter (`fp(r) == fp(v)`) can no longer match and becomes `retire: vec![]`.
  - `transfer::successor`: the position of a rescue whose `predecessor` is the vault's and which is not landed. `newest_live` reaches rescues through `successor` and through its live-fingerprint match; apply `landed` in both, so the rule lives in these two functions, never in `export.rs`.
- *Log lines* (this plan fixes the text; tests match on it):
  - `tracing::debug!(account = %row.id, fp = %fp.short12(), "wrote a refreshed token ahead to rescue/ until the vault takes it")`
  - `tracing::warn!(position = row.position, account = %row.id, kind = e.kind(), "could not write a refreshed token ahead to rescue/; the vault write goes on without it")`
  - `tracing::warn!(position = …, account = …, kind = e.kind(), "a rescue written ahead could not be deleted after the vault took its token")`
  - `tracing::debug!(position = row.position, account = %row.id, "deleted a rescue the vault already holds")` and, on failure, `tracing::warn!(…, kind = err.kind(), "a rescue the vault already holds could not be deleted")`

- [ ] **Step 1: Write the failing tests**

  `crates/tagteam-engine/tests/gate_persist.rs`, top level (add `capture_logs`, `credential`, `rescue_files`, `token_requests` to the `common` import):
  ```rust
  #[test]
  fn a_same_generation_rescue_is_deleted_and_never_adopted_over_a_newer_access_token() {
      let fx = Fx::new();
      let a = due(&fx);
      let snapshot = fx.vault_bytes(&a).unwrap();
      fx.script_refresh(None); // no refresh token in the reply: the successor keeps the fingerprint
      fx.kc.set_fail_write(SERVICE, true);
      let out = refresh(&fx, &a, &snapshot);
      fx.kc.set_fail_write(SERVICE, false);
      assert!(matches!(&out, GateOutcome::Transient { kind, rescued: true } if kind == "vault-write"), "{out:?}");
      assert_eq!(rescue_files(&fx), 1);
      // Another writer stores a newer access token of the same generation meanwhile.
      let mut newer: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
      newer["claudeAiOauth"]["accessToken"] = json!("at-newer");
      newer["claudeAiOauth"]["expiresAt"] = json!(fx.clock.now_ms() + 3_600_000);
      fx.put_vault(&a, newer.to_string().as_bytes());
      let out = refresh(&fx, &a, &snapshot);
      assert!(matches!(out, GateOutcome::AlreadyFresh(_)), "{out:?}");
      assert_eq!(rescue_files(&fx), 0);
      let stored: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
      assert_eq!(stored["claudeAiOauth"]["accessToken"], "at-newer", "the rescue's at-same never went over it");
      assert_eq!(token_requests(&fx), 1);
  }

  #[test]
  fn a_failed_write_ahead_is_logged_and_the_vault_still_takes_the_successor() {
      let fx = Fx::new();
      let a = due(&fx);
      let snapshot = fx.vault_bytes(&a).unwrap();
      fx.script_refresh(Some("rt-a2"));
      block_rescue(&fx);
      let (out, logs) = capture_logs(|| refresh(&fx, &a, &snapshot));
      unblock_rescue(&fx);
      assert!(matches!(out, GateOutcome::Refreshed(_)), "{out:?}");
      assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
      assert_eq!(rescue_files(&fx), 0);
      assert!(logs.iter().any(|l| l.contains("WARN") && l.contains("could not write a refreshed token ahead to rescue/")), "{logs:#?}");
  }

  #[test]
  fn a_rescue_written_before_this_branch_still_settles_into_the_vault_form() {
      // Review Focus line 2, rescue half: a pre-branch envelope holds full bytes, `mcpOAuth` included.
      let fx = Fx::new();
      let a = due(&fx);
      let snapshot = fx.vault_bytes(&a).unwrap();
      fx.plant_rescue(&a, &vault_fp(&fx, &a), &credential("a@x.co", "rt-a2"));
      let out = refresh(&fx, &a, &snapshot);
      assert!(matches!(out, GateOutcome::AlreadyFresh(_)), "{out:?}");
      let stored: Value = serde_json::from_slice(&fx.vault_bytes(&a).unwrap()).unwrap();
      assert_eq!(stored["claudeAiOauth"]["refreshToken"], "rt-a2");
      assert!(stored.get("mcpOAuth").is_none(), "adopted in the vault form");
      assert_eq!(rescue_files(&fx), 0);
      assert_eq!(token_requests(&fx), 0);
  }
  ```
  `gate_persist.rs` `hooked` (in-process, a kill is replayed as the disk it leaves: an error injected where the vault write would start runs no `keep` that writes, which the unchanged inode proves):
  ```rust
  /// `(path, inode, envelope)` of every file in `rescue/`, by path (a type alias keeps clippy's
  /// `type_complexity` quiet).
  type Listing = Vec<(PathBuf, u64, Value)>;
  fn listing(dir: &Path) -> Listing { /* fs::read_dir, MetadataExt::ino, parse; empty when absent */ }

  /// What `rescue/` held each time the engine passed `point`.
  fn rescues_at(fx: &Fx, point: &'static str) -> Arc<Mutex<Vec<Listing>>> {
      let seen: Arc<Mutex<Vec<_>>> = Arc::default();
      let (slot, dir) = (seen.clone(), fx.env.data_dir().join("rescue"));
      fx.engine.on_point(point, Box::new(move || slot.lock().unwrap().push(listing(&dir))));
      seen
  }

  #[test]
  fn an_own_successor_is_written_ahead_before_the_vault_re_read_and_deleted_once_stored() {
      let fx = Fx::new();
      let a = due(&fx);
      let sent = vault_fp(&fx, &a);
      let snapshot = fx.vault_bytes(&a).unwrap();
      fx.script_refresh(Some("rt-a2"));
      let seen = rescues_at(&fx, "gate-after-response"); // before persist_successor's re-read
      let (out, logs) = capture_logs(|| refresh(&fx, &a, &snapshot));
      assert!(matches!(out, GateOutcome::Refreshed(_)), "{out:?}");
      let seen = seen.lock().unwrap();
      assert_eq!(seen[0].len(), 1, "written ahead");
      assert_eq!(seen[0][0].2["predecessorFp"], json!(sent));
      assert!(rescues(&fx).is_empty(), "deleted once the vault write verified");
      assert!(logs.iter().any(|l| l.contains("DEBUG") && l.contains("wrote a refreshed token ahead to rescue/")));
      assert!(!logs.iter().any(|l| l.contains("kept a refreshed token in rescue/")), "routine: no INFO line");
  }

  #[test]
  fn a_crash_between_the_write_ahead_and_the_vault_write_is_adopted_by_the_next_gate_pass() {
      let fx = Fx::new();
      let a = due(&fx);
      let sent = vault_fp(&fx, &a);
      let snapshot = fx.vault_bytes(&a).unwrap();
      fx.script_refresh(Some("rt-a2"));
      let seen = rescues_at(&fx, "gate-after-response");
      fx.engine.fail_at(Some("gate-before-vault-write"));
      assert!(fx.engine.refresh_stored(fx.cc.as_ref(), &a, &snapshot).is_err());
      fx.engine.fail_at(None);
      assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a"), "the vault write never started");
      assert_eq!(listing(&fx.env.data_dir().join("rescue")), seen.lock().unwrap()[0], "no second copy");
      assert_eq!(rescued_refresh_tokens(&fx), ["rt-a2"]);
      assert_eq!(rescues(&fx)[0]["predecessorFp"], json!(sent));
      let out = refresh(&fx, &a, &snapshot); // step 3 adopts it; no second request
      assert!(matches!(out, GateOutcome::AlreadyFresh(_)), "{out:?}");
      assert_eq!(fx.vault_refresh_token(&a).as_deref(), Some("rt-a2"));
      assert!(rescues(&fx).is_empty());
      assert_eq!(token_requests(&fx), 1);
  }

  #[test]
  fn a_crash_after_the_vault_write_leaves_a_rescue_the_landed_rule_deletes() {
      // Replayed by putting back the envelope `gate-before-vault-write` saw, once the vault holds it.
      let fx = Fx::new();
      let a = due(&fx);
      let snapshot = fx.vault_bytes(&a).unwrap();
      fx.script_refresh(Some("rt-a2"));
      let seen = rescues_at(&fx, "gate-before-vault-write");
      assert!(matches!(refresh(&fx, &a, &snapshot), GateOutcome::Refreshed(_)));
      let (landed, prev) = (fx.vault_bytes(&a).unwrap(), fx.kc.get(SERVICE, &format!("{a}.prev")));
      for (path, _, envelope) in &seen.lock().unwrap()[0] {
          fs::write(path, envelope.to_string()).unwrap();
      }
      assert_eq!(rescue_files(&fx), 1);
      let out = refresh(&fx, &a, &snapshot);
      assert!(matches!(out, GateOutcome::AlreadyFresh(_)), "{out:?}");
      assert_eq!(rescue_files(&fx), 0, "deleted by the landed rule");
      assert_eq!(fx.vault_bytes(&a).unwrap(), landed, "never adopted again");
      assert_eq!(fx.kc.get(SERVICE, &format!("{a}.prev")), prev, ".prev did not rotate");
      assert_eq!(token_requests(&fx), 1);
  }

  #[test]
  fn a_failed_vault_write_keeps_the_envelope_written_ahead_and_writes_no_second_copy() {
      let fx = Fx::new();
      let a = due(&fx);
      let snapshot = fx.vault_bytes(&a).unwrap();
      fx.script_refresh(Some("rt-a2"));
      let seen = rescues_at(&fx, "gate-after-response");
      fx.kc.set_fail_write(SERVICE, true);
      let (out, logs) = capture_logs(|| refresh(&fx, &a, &snapshot));
      fx.kc.set_fail_write(SERVICE, false);
      assert!(matches!(&out, GateOutcome::Transient { kind, rescued: true } if kind == "vault-write"), "{out:?}");
      assert_eq!(listing(&fx.env.data_dir().join("rescue")), seen.lock().unwrap()[0], "same file, same inode");
      assert_eq!(logs.iter().filter(|l| l.contains("kept a refreshed token in rescue/ until the vault takes it")).count(), 1, "{logs:#?}");
  }

  #[test]
  fn a_foreign_successor_is_never_written_ahead() {
      let fx = Fx::new();
      let a = due(&fx);
      let snapshot = fx.vault_bytes(&a).unwrap();
      fx.http.push_json(Method::Post, &Fx::endpoints().token, 200,
          json!({"access_token": "at-a2", "refresh_token": "rt-a2", "expires_in": 28800,
                 "account": {"uuid": "uuid-someone-else"}}));
      let seen = rescues_at(&fx, "gate-after-response");
      let out = refresh(&fx, &a, &snapshot);
      assert!(matches!(out, GateOutcome::Dead(_)), "{out:?}");
      assert!(seen.lock().unwrap()[0].is_empty());
      assert!(rescues(&fx).is_empty());
      assert_eq!(fx.displaced().len(), 1);
  }
  ```
  `tests/active.rs` (the live account; `fp`, `bytes`, `cred`, `active` are the file's helpers):
  ```rust
  #[test]
  fn a_same_generation_rescue_of_the_live_login_is_deleted_and_never_published() {
      let fx = Fx::new();
      let a = fx.add("a@x.co", "rt-a");
      let vault = fx.vault_bytes(&a).unwrap();
      let mut same = Fx::credential_json("a@x.co", "rt-a");
      same["claudeAiOauth"]["accessToken"] = json!("at-crash");
      fx.plant_rescue(&a, &fp(&fx, &vault), &bytes(&same));
      let out = active(&fx, ActiveTrigger::Expired).unwrap();
      assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
      assert_eq!(rescue_files(&fx), 0);
      assert_eq!(fx.live_credential().unwrap()["claudeAiOauth"]["accessToken"], "at-rt-a", "never published");
      assert_eq!(fx.vault_bytes(&a).unwrap(), vault, "never adopted");
      assert_eq!(token_requests(&fx), 0);
  }

  #[cfg(feature = "test-hooks")]
  #[test]
  fn an_active_refresh_interrupted_after_its_write_ahead_leaves_a_rescue_the_next_pass_deletes() {
      let fx = Fx::new();
      fx.add("a@x.co", "rt-a");
      let rejected = fx.cc.access_fingerprint(&cred("a@x.co", "rt-a")).unwrap().as_str().to_owned();
      fx.script_refresh(None); // same generation, `at-same`
      fx.engine.fail_at(Some("active-after-response"));
      assert!(active(&fx, ActiveTrigger::Rejected { access_fp: rejected }).is_err());
      fx.engine.fail_at(None);
      assert_eq!(rescue_files(&fx), 1, "the write-ahead; `keep` wrote no second copy");
      let out = active(&fx, ActiveTrigger::Expired).unwrap();
      assert_eq!(out, ActiveOutcome::NotNeeded { reconciled: true });
      assert_eq!(rescue_files(&fx), 0);
      assert_eq!(fx.live_credential().unwrap()["claudeAiOauth"]["accessToken"], "at-rt-a");
  }
  ```
  `tests/export.rs` (`fp_of`, `running`, `all`, `envelope`, `sources` are the file's helpers; add `ExportResult` to its imports if missing):
  ```rust
  #[test]
  fn a_landed_rescue_is_skipped_by_the_live_session_and_idle_export_paths() {
      let landed = |fx: &Fx, id: &AccountId, email: &str, rt: &str| {
          let mut same = Fx::credential_json(email, rt);
          same["claudeAiOauth"]["accessToken"] = json!("at-crash");
          fx.plant_rescue(id, &fp_of(fx, email, rt), same.to_string().as_bytes());
      };
      let at = |r: &ExportResult, position: u32| {
          envelope(r).iter().find(|a| a["position"] == position).unwrap()["credential"]["claudeAiOauth"]["accessToken"].clone()
      };
      // The live login: `transfer::newest_live` picks the vault and leaves the file (pickers only read).
      let fx = Fx::new();
      two_accounts(&fx);
      let b = fx.engine.resolve("2", None).unwrap().id;
      landed(&fx, &b, "b@x.co", "rt-b");
      let r = all(&fx);
      assert_eq!((sources(&r)[1], at(&r, 2)), ((2, Source::Vault, true), json!("at-rt-b")));
      assert_eq!(rescue_files(&fx), 1);
      // A session-owned account: `transfer::successor`.
      let fx = Fx::new();
      let a = two_accounts(&fx);
      let _session = running(&fx, &a, "rt-a", "rt-a");
      landed(&fx, &a, "a@x.co", "rt-a");
      let r = all(&fx);
      assert_eq!((sources(&r)[0], at(&r, 1)), ((1, Source::Vault, true), json!("at-rt-a")));
      assert_eq!(rescue_files(&fx), 1);
      // An idle account: `settle_rescues`, an adopter, deletes it before the vault is exported.
      let fx = Fx::new();
      let a = two_accounts(&fx);
      landed(&fx, &a, "a@x.co", "rt-a");
      assert_eq!(at(&all(&fx), 1), "at-rt-a");
      assert_eq!(rescue_files(&fx), 0);
  }
  ```
  `src/transfer.rs` `mod tests`:
  ```rust
  #[test]
  fn a_rescue_the_vault_already_holds_is_never_picked() {
      let v = fp("v");
      let landed = rescue("v", "v"); // its own generation is the vault's
      assert_eq!(successor(&v, std::slice::from_ref(&landed)), None);
      assert_eq!(successor(&v, &[landed.clone(), rescue("r", "v")]), Some(1));
      for live in [generation("v", true), generation("prev", true)] {
          assert_eq!(newest_live(&live, &v, Some(&fp("prev")), std::slice::from_ref(&landed)), Pick::Vault, "{live:?}");
      }
  }
  ```
  `src/rescue.rs` `mod tests`:
  ```rust
  fn same_generation(rt: &str) -> Vec<u8> {
      json!({"claudeAiOauth": {"accessToken": "at-other", "refreshToken": rt, "expiresAt": 1,
          "refreshTokenExpiresAt": 5}}).to_string().into_bytes()
  }

  #[test]
  fn a_rescue_the_vault_already_holds_is_deleted_never_adopted() {
      let t = T::new();
      let row = t.account("a@x.co", &cred("rt-1", 5));
      let vault = t.kc.get(SERVICE, row.id.as_str()).unwrap();
      let same = same_generation("rt-1");
      let path = t.engine.write_rescue(t.cc.as_ref(), &row.id, 0, &t.fp(&vault), &same, &t.cc.fingerprint(&same).unwrap()).unwrap();
      let lock = t.lock(&row.id);
      t.engine.settle_rescues(t.cc.as_ref(), &row, &lock).unwrap();
      assert!(!path.exists());
      assert_eq!(t.kc.get(SERVICE, row.id.as_str()).unwrap(), vault);
      assert_eq!(t.vault_rt(&row.id, true), None, "no .prev");
  }

  #[test]
  fn a_landed_rescue_that_cannot_be_deleted_is_still_never_adopted() {
      // as above, then `rescue/` made 0500 before `settle_rescues` (restore 0700 after):
      // Ok(()), the file is still there, and the vault bytes are unchanged.
  }
  ```
  `crates/tagteam/tests/gate_race.rs` (needs `--features test-support`, as the file already does; add `use std::fs;` and `tagteam_provider::Env`):
  ```rust
  #[test]
  fn a_kill_after_the_write_ahead_leaves_the_successor_in_rescue_and_the_next_switch_adopts_it() {
      // H1.1 criterion 3 with a real kill: `TAGTEAM_TEST_CRASH_AT` exits there with 137, so no
      // destructor, `Received::drop` included, runs. Only the write-ahead can have kept the token.
      let d = tempfile::tempdir().unwrap();
      let root = d.path();
      let a = expiring_target(root);
      let server = MockServer::start();
      server.on("POST", TOKEN, token_reply("rt-a2"));
      cmd(root).env(API_BASE, server.base_url()).env("TAGTEAM_TEST_CRASH_AT", "gate-before-vault-write")
          .args(["switch", "1", "--json"]).assert().code(137);
      assert_eq!(vault_refresh_token(root, &a), "rt-a");
      let dir = Env::for_test(root).data_dir().join("rescue");
      let files: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
      assert_eq!(files.len(), 1);
      let envelope: Value = serde_json::from_slice(&fs::read(&files[0]).unwrap()).unwrap();
      let kept: Value = serde_json::from_str(envelope["credential"].as_str().unwrap()).unwrap();
      assert_eq!(kept["claudeAiOauth"]["refreshToken"], "rt-a2");
      cmd(root).env(API_BASE, server.base_url()).args(["switch", "1", "--json"]).assert().success();
      assert_eq!(server.hits("POST", TOKEN), 1, "adopted, never requested again");
      assert_eq!(vault_refresh_token(root, &a), "rt-a2");
      assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
      assert_eq!(live_email(root), "a@x.co");
  }
  ```

- [ ] **Step 2: Run them to verify they fail**

  - `cargo test -p tagteam-engine --features test-hooks --test gate_persist --test active --test export` → fail: the write-ahead tests (nothing in `rescue/` at `gate-after-response`, no DEBUG line; the crash test's listing differs because `keep` wrote the file after the point; no WARN for the failed write-ahead), the same-generation, landed-crash, live and picker tests (the rescue is adopted or picked: `at-same`/`at-crash` where the vault's access token is expected, or a rescue left behind). `a_foreign_successor_is_never_written_ahead` and the Review Focus test pass already: they pin what does not change (the latter needs Task 8).
  - `cargo test -p tagteam-engine --lib -- rescue::tests transfer::tests` → the landed tests fail (adopted; `Some(0)` instead of `None`).
  - `cargo test -p tagteam --features test-support --test gate_race a_kill_after` → fails: `rescue/` does not exist after the kill.

- [ ] **Step 3: Implement**

  As "The rules, as built" above: `ahead: Option<PathBuf>` on `Received`; the write-ahead at the end of `Received::new` (before `Self` is returned, after `fp` is known); `keep`'s own-successor branch writes only without `ahead`, then logs the INFO line; `landed` replaces `disarm` in `persist_received`; `write_rescue` no longer logs; the landed rule in `settle_rescues`, `reconcile_active`, `successor` and `newest_live`. Update `Received`'s, `persist_received`'s, `write_rescue`'s and `settle_rescues`'s doc comments (§6.3 as amended: `rescue/` is also written ahead of every vault write of an own successor).

- [ ] **Step 4: Run them to verify they pass**

  - `cargo test -p tagteam-engine --features test-hooks` and `cargo test -p tagteam --features test-support --test gate_race` → PASS (MockServer binds 127.0.0.1, which works in the sandbox). Every test above compiled and passed on a prototype, and the landed-rule tests failed with the rule switched off; clippy is clean with and without `test-support`.
  - `cargo test --workspace --locked --features tagteam/test-support` → PASS apart from the sandbox's usual pty and socket failures. No existing test pins the old behaviour: a prototype of this task on top of Task 8 passed every test that passes in the sandbox at 5c9a114, `gate_persist.rs`'s `a_received_successor_is_never_discarded` fault matrix included (with `rescue/` blocked the write-ahead fails and `keep` fails as before, so the `Unpersisted` rows are unchanged).

- [ ] **Step 5: Commit**

  The files above — "Write a refreshed token to rescue/ ahead of the vault and delete rescues the vault already holds"

---

### Task 10: The notifier blocks until a notice or its stop (H8)

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-daemon/src/notify.rs` (the `mpsc` import, :10; `POLL`, :49-50, removed; `NoticeQueue` and `for_test`, :106-130; `Notifier` and its `Drop`, :132-172; `deliver_until_stopped`, :174-192)
- Modify: `crates/tagteam-daemon/src/testutil.rs` (append `within` after `capture_logs`, :537-550)
- Test: `crates/tagteam-daemon/src/notify.rs` `mod tests` (import `within`, :515-517; one test after `a_stopped_notifier_delivers_nothing_more`, :745-754); `crates/tagteam-daemon/src/daemon.rs` `mod tests` only (import `within`, :429; one test at the end, before :1082). No production line of `daemon.rs` changes.

**Interfaces:**
- Consumes: nothing.
- Produces (all crate-private; `NoticeQueue::send`, `Notifier::start`, `Notifier::stop` keep their signatures):
  - `enum Message { Notice(Notice), Stop }` (private to `notify.rs`), the channel's item: `NoticeQueue { tx: SyncSender<Message> }`.
  - `Notifier { stop: Arc<AtomicBool>, tx: SyncSender<Message>, thread: Option<JoinHandle<()>> }`.
  - `#[cfg(test)] NoticeQueue::for_test() -> (NoticeQueue, Queued)`, with `#[cfg(test)] pub(crate) struct Queued(Receiver<Message>)` and `pub(crate) fn try_iter(&self) -> impl Iterator<Item = Notice> + '_` (the notices, in order). Every existing caller (`rx.try_iter().collect()`, `.count()`; all in `notify.rs`) compiles unchanged.
  - `testutil`: `pub(crate) fn within<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T` — runs `f` on a thread of its own and waits 10 s on a channel: a timeout panics `"not within 10 s: {what}"`, a panic in `f` is resumed (`resume_unwind` of the joined thread's payload). A thread that never wakes then fails the test instead of stalling the run.

Why the stop is a message and the flag stays: `Started.notifier` is `Option<(Notifier, NoticeQueue)>` (`daemon.rs:247`, the struct's last field); a tuple drops its elements in order, so a `Started` dropped without `serve` runs `Notifier::drop` (which joins) while its `NoticeQueue` still holds a sender. A thread in `recv()` would never see the channel disconnect: verified on a prototype, where dropping without a stop message hangs both tests below. The stop message wakes the thread; the `AtomicBool` still decides, because `Stop` queues behind any waiting notices and D6.2 delivers none of them after a stop. Rejected: `try_send(Stop)` ignoring `Full` (correct, but a full queue's stop would rest on the thread seeing the flag; a blocking `send` never does).

- [ ] **Step 1: Write the failing tests**

  `notify.rs` `mod tests`:
  ```rust
  #[test]
  fn a_notifier_dropped_while_its_queue_lives_stops_at_once() {
      // H8, D6: the thread blocks in `recv`, and `Notifier::drop` wakes it with its stop
      // message. `Started` drops its `Notifier` before the queue beside it (`daemon.rs:247`), so
      // each queue is held across its notifier's drop. Ten drops took about a second while the
      // thread polled every 100 ms.
      let took = within("ten notifiers dropped", || {
          let spawner = Arc::new(ScriptedSpawner::new());
          let mut took = Duration::ZERO;
          for _ in 0..10 {
              let (notifier, queue) = Notifier::start(Backend::Osascript, spawner.clone());
              let started = Instant::now();
              drop(notifier);
              took += started.elapsed();
              drop(queue);
          }
          took
      });
      assert!(took < Duration::from_millis(500), "{took:?} for ten drops");
  }
  ```

  `daemon.rs` `mod tests`:
  ```rust
  #[test]
  fn a_start_dropped_without_serving_ends_its_notifier_with_notices_off() {
      // H8, D6.2: on macOS the notifier starts whatever `notify.enabled` says (unchanged); idle,
      // it now costs no wakeup. A `Started` dropped without `serve` drops the `Notifier` before
      // the `NoticeQueue` it holds, so only the stop message can end the thread.
      within("a start dropped without serving", || {
          let home = Fx::new();
          home.settings("[notify]\nenabled = false\n", 1);
          let opts = DaemonOptions {
              backend: Some(Backend::Osascript),
              ..options(&home)
          };
          let started = Daemon::start(&home.engine, opts, &mut |n| panic!("{n}")).unwrap();
          assert!(!home.engine.settings().notify_enabled);
          assert!(started.notifier.is_some(), "the notifier starts with notices off");
          drop(started);
          assert!(record(&home).is_empty(), "the rest of the start was let go too");
      });
  }
  ```

- [ ] **Step 2: Run them to verify they fail**

  `cargo test -p tagteam-daemon --lib -- a_notifier_dropped_while_its_queue_lives_stops_at_once a_start_dropped_without_serving_ends_its_notifier_with_notices_off` (runs inside the sandbox). Expected: the `notify` test fails its bound, about 1 s for ten drops (1.04 s measured at 5c9a114). The `daemon` test passes against today's code: it is the guard against the hang that a `recv()` without the stop message would cause (with that mistake, both tests fail `not within 10 s`).

- [ ] **Step 3: Implement** (`notify.rs`)
  - `NoticeQueue::send` `try_send`s `Message::Notice(n)`; its `Full` and `Disconnected` handling is unchanged.
  - `Notifier::start` keeps a clone of the sender in `Notifier.tx`.
  - `Drop for Notifier`, in this order: store the flag (`SeqCst`); `let _ = self.tx.send(Message::Stop)` (blocking: with a full queue, room comes once the delivery in flight ends, which the join waits for anyway; `Err` means the thread has gone); join.
  - `deliver_until_stopped` takes `&Receiver<Message>` and loops on `rx.recv()`: any message once the flag is set returns (nothing still queued is delivered, D6.2); `Message::Notice` is delivered as now; `Message::Stop` or `Err(RecvError)` returns.
  - Remove `POLL` and `RecvTimeoutError`. `Notifier`'s doc says the thread blocks until a notice or the stop message `drop` sends (H8).

- [ ] **Step 4: Run them to verify they pass**

  `cargo test -p tagteam-daemon --lib` → PASS, 133 tests (131 at 5c9a114, plus these two). No existing test pins the 100 ms poll. Unchanged and still passing: `a_failed_delivery_is_logged_once_at_warn_then_at_debug_and_the_next_still_goes` (notices are still delivered through the real thread), `a_stopped_notifier_delivers_nothing_more`, `a_full_queue_drops_a_notice_rather_than_wait`, and `daemon::tests::a_switch_while_serving_is_announced_and_delivered_through_the_notifier`.

- [ ] **Step 5: Commit**

  `crates/tagteam-daemon/src/notify.rs`, `crates/tagteam-daemon/src/daemon.rs`, `crates/tagteam-daemon/src/testutil.rs`: "Block the notifier until a notice or its stop message".

---

### Task 11: Accept and connection threads wake on their descriptors (H8)

**Depends on:** 10

**Files:**
- Modify: `crates/tagteam-daemon/src/server.rs`:
  - module doc (:1-7) and imports (:10, :12)
  - `POLL` (:48-50), which becomes `RETRY`
  - `Limits::write_timeout`'s doc (:61-62)
  - `Shared` (:93-106), `bind_with` (:121-161) and `close` (:176-186)
  - `accept_loop` and its doc (:227-245), and `accept_pending`'s sleep (:257)
  - `pending` (:264-279), replaced by `poll`, `ready` and `pollfd`, with `Wake` and `set_nonblocking` new beside them
  - `Hub` (:380-388), `Subscriber` (:396-401), `Hub::new` (:404-412), `close_socket` (:424-438), `queue` (:441-464) and `subscribe` (:467-474)
  - `Connection` (:509-518), `new` (:521-534), `run` (:537-577), `request`'s subscribe arm (:602-611), `flush` and `send` (:622-636), and `Drop` (:639-646)
- Test: `crates/tagteam-daemon/tests/server.rs` (a local `within` helper and three tests appended after :604; the leading comment of `a_signal_stops_new_connections_but_subscribers_still_hear_stopping`, :564-566). `tests/common/mod.rs` and `tests/client.rs` are not touched.

**Interfaces:**
- Consumes: nothing from Task 10. The dependency is only the order: one change to the daemon crate at a time.
- Produces: no public change. `Server::bind`, `bind_with`, `shutdown`, `hub`, `Limits`, `IDLE`, `WRITE_TIMEOUT`, `QUEUE`, `MAX_CONNECTIONS`, `Hub` (a `DaemonSink`, `Send + Sync`) and `peer_uid` keep their signatures and values. Private to `server.rs`:
  - `struct Wake { read: PipeReader, write: PipeWriter }`, with these methods:
    - `fn new() -> io::Result<Wake>`
    - `fn wake(&self)`: writes one byte; a full pipe (`WouldBlock`) and any error are ignored
    - `fn drain(&self)`: reads until `Ok(0)`, `WouldBlock` or an error
    - `fn fd(&self) -> RawFd`: the read end
  - `fn set_nonblocking(fd: BorrowedFd<'_>) -> io::Result<()>`
  - `fn pollfd(fd: RawFd, events: libc::c_short) -> libc::pollfd`
  - `fn poll(fds: &mut [libc::pollfd], timeout: Option<Duration>) -> io::Result<usize>`: `None` waits with no bound; a timeout is rounded up to a whole millisecond, so a wait never spins; `EINTR` is returned as `ErrorKind::Interrupted`
  - `fn ready(fd: &libc::pollfd) -> bool`: `revents` has `POLLIN | POLLHUP | POLLERR | POLLNVAL`
  - `Shared.stop_wake: Wake` and `Hub.closed_wake: Wake`, both latches: written once their flag is set, never drained
  - `Subscriber.wake: Arc<Wake>`; `Hub::new(board: Arc<StatusBoard>, file: Option<Bound>, closed_wake: Wake)`
  - `Hub::subscribe(&self, socket: UnixStream, wake: Arc<Wake>) -> (u64, Receiver<Arc<str>>)`
  - `Connection.subscription: Option<(u64, Receiver<Arc<str>>, Arc<Wake>)>`
  - `Connection::write_all(&self, bytes: &[u8]) -> io::Result<()>`

Choices this task commits to:
- **Pipes from `std::io::pipe()`** (stable since 1.87; MSRV 1.88). std makes both ends close-on-exec: `pipe2(O_CLOEXEC)` on Linux; on macOS, which has no `pipe2`, `pipe` then `FD_CLOEXEC`. `set_nonblocking` (fcntl `F_GETFL`/`F_SETFL | O_NONBLOCK`) is the one new `unsafe` block. Its SAFETY comment follows the file's style, e.g. "fcntl(2) with F_GETFL and F_SETFL only reads and sets the status flags of a descriptor that `fd` keeps open". Rejected: `libc::pipe`/`pipe2` by hand, which needs more unsafe for the same result. Also rejected: `UnixStream::pair()`, which needs no new unsafe but is a socket pair, where H8 says pipes.
- **Write waits are `poll` timeouts, as H8 says.** The accepted socket is made non-blocking (macOS inherits the listener's flag, Linux does not), and `SO_RCVTIMEO`/`SO_SNDTIMEO` are no longer set. `write_all` writes; on `WouldBlock` it polls `POLLOUT` until `limits.write_timeout` from that moment (`EINTR` retries within the same deadline). A timeout gives `ErrorKind::TimedOut`, and `Ok(0)` from `write` gives `WriteZero`. The thread then never blocks outside `poll`, so a read after a spurious readiness cannot hang it. Rejected: a blocking socket keeping `SO_SNDTIMEO`. The write wait never watches the stop descriptor (Decision 9: a connection writes what it holds, and `shutdown` waits for a client that is not reading for at most the write timeout, as today). A full-queue subscriber's wait ends when the hub shuts its socket down: on macOS this was checked on a socketpair, where `poll(POLLOUT)` returned `POLLHUP` and the write `EPIPE`. On Linux it follows from `unix_poll`, which sets `POLLHUP` once both directions are shut down; Linux CI covers it.
- **Rejected larger design:** one event-loop thread polling every connection. The wakeup goal needs only each thread to block on its own descriptors.

- [ ] **Step 1: Write the failing tests** (`tests/server.rs`)

  The helper is `testutil::within` from Task 10, with the same 10 s bound. It is copied here because integration tests cannot see the crate's `cfg(test)` module:
  ```rust
  /// Runs `f` on a thread of its own and waits up to 10 s for it, so a thread that never wakes
  /// fails the test instead of stalling the run. A panic in `f` is the test's.
  fn within<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T
  ```

  ```rust
  #[test]
  fn shutdown_wakes_the_accept_thread_and_a_connection_at_once() {
      // H8: the accept thread waits in `poll` on the listener and the hub's wake pipe, and a
      // connection on its socket and the server-stop pipe, all with no timeout. `shutdown`
      // writes both pipes (the hub's through `close_socket`). Ten shutdowns, each with a
      // greeted connection, took about a second under the 100 ms polls.
      let dir = short_tempdir();
      let took = within("ten shutdowns", move || {
          let path = dir.path().join("daemon.sock");
          let mut took = Duration::ZERO;
          for _ in 0..10 {
              let (server, _board) = start(dir.path());
              let mut peer = Peer::greeted(&path);
              let started = Instant::now();
              server.shutdown();
              took += started.elapsed();
              assert!(peer.next_line().is_none());
          }
          took
      });
      assert!(took < Duration::from_millis(500), "{took:?} for ten shutdowns");
  }

  #[test]
  fn an_event_reaches_a_subscriber_at_once() {
      // H8: `Hub::queue` writes the subscriber's wake pipe after each `try_send`. One event at a
      // time, each emitted once the last has arrived: under the 100 ms read timeout each waited
      // about 100 ms, so twenty took about two seconds.
      let dir = short_tempdir();
      let (server, _board) = start(dir.path());
      let mut peer = Peer::subscribed(&dir.path().join("daemon.sock"), "s");
      let started = Instant::now();
      for n in 0..20 {
          server.hub().emit(&burst_event(n));
          assert_eq!(peer.line()["message"].as_str().unwrap()[..6], format!("{n:06}"));
      }
      let took = started.elapsed();
      assert!(took < Duration::from_secs(1), "{took:?} for twenty events");
      server.shutdown();
  }
  ```

  The Review Focus line 4 test is built as Review Focus 2's `a_stalled_subscriber_is_dropped_alone_and_the_rest_see_every_event` (:241-322) is. It has a 60 s write timeout, batches of 32, and readers that count what they read. Ten readers stand in for that test's fifteen, and five greeted-only connections fill the sixteen:
  ```rust
  #[test]
  fn shutdown_with_a_full_queue_and_sixteen_connections_ends_every_thread_and_the_rest_read_stopping() {
      // Review Focus 4, Decision 9, H8: sixteen connections. One subscriber never reads, ten
      // read, and five only greeted. A burst fills the stalled one's queue. With no read timeout
      // left, only descriptors end the threads:
      // - the stalled one waits for room under a 60 s write timeout until the hub shuts its
      //   socket on the full queue
      // - the rest end on the server-stop pipe; the greeted ones would otherwise wait out 30 s
      //   idle, and the readers forever
      const READERS: usize = 10;
      const BATCH: usize = 32;
      // setup as Review Focus 2's test, except that each reader thread, on `stopping`, returns
      // `(seen, peer.next_line())`, and these connections are added:
      let mut greeted: Vec<Peer> = (0..MAX_CONNECTIONS - READERS - 1)
          .map(|_| Peer::greeted(&path))
          .collect();
      assert_eq!(board.snapshot().subscribers, READERS + 1);
      let mut emitted = 0;
      while board.snapshot().subscribers == READERS + 1 {
          assert!(emitted < 16 * QUEUE, "the stalled subscriber was never dropped");
          // BATCH events, then wait_until every reader's count == emitted (as Review Focus 2)
      }
      server.hub().emit(&DaemonEvent::Stopping { reason: StopReason::Signal });
      let took = within("shutdown", move || {
          let started = Instant::now();
          server.shutdown();
          started.elapsed()
      });
      assert!(took < Duration::from_secs(5), "{took:?}");
      let want: Vec<String> = (0..emitted).map(|n| format!("{n:06}")).collect();
      for reader in readers {
          let (seen, after) = reader.join().unwrap();
          assert_eq!(seen, want);
          assert!(after.is_none(), "{after:?} after `stopping`");
      }
      for peer in &mut greeted {
          assert!(peer.rest().is_empty());
      }
      assert!(stalled.rest().len() < emitted);
      assert_eq!(board.snapshot().subscribers, 0);
  }
  ```
  The stalled queue is found full by whichever emit fills it. In a real shutdown that emit is `stopping`; here it is a burst event just before `stopping`, through the same `Hub::queue` path. "Exactly full at `stopping`" cannot be staged: the kernel's socket buffer decides how many events the stalled thread writes before it waits (8 KiB on macOS, about 208 KiB on Linux).

- [ ] **Step 2: Run them to verify they fail**

  Outside the sandbox (binding a Unix socket fails there with EPERM):
  `cargo test -p tagteam-daemon --test server -- shutdown_wakes_the_accept_thread_and_a_connection_at_once an_event_reaches_a_subscriber_at_once shutdown_with_a_full_queue_and_sixteen_connections`.
  Expected: the first two fail their bounds, measured at 5c9a114 as `1.02s for ten shutdowns` and `2.02s for twenty events`. The Review Focus 4 test passes against today's code. It is the guard: without the server-stop wake it fails `not within 10 s: shutdown`, and so does the first test (checked on a prototype).

- [ ] **Step 3: Implement** (`server.rs`)
  - **`bind_with`:** after the `TooLong` check and before `bind_private`, make `stop_wake` and `closed_wake` (`Wake::new()?`, mapped to `BindError::Io`). A failure then leaves no socket file. The pipes create no file, so the umask window is untouched.
  - **`Server::close`:**
    1. set `stop`
    2. `stop_wake.wake()`
    3. `hub.close_socket()`
    4. join the accept thread, then the connections, as now
  - **`Hub::close_socket`:** set `closed`, then `closed_wake.wake()`, both before the early returns. The file is still removed only once, and only while it is the one `bind` made.
  - **`Hub::queue`:** `s.wake.wake()` after each `Ok(())` of `try_send`, under the subscribers lock (the write never blocks). The full-queue and disconnected arms are unchanged.
  - **`accept_loop`:** the loop's test (stop, `accepting()`, `cancel.requested()`) is unchanged. Each pass polls `[listener POLLIN, closed_wake POLLIN]` with no timeout:
    - wake ready: accept nothing, and the loop's test returns
    - else listener ready: `accept_pending`
    - `Interrupted`: loop again
    - any other error: DEBUG and `sleep(RETRY)`; `accept_pending`'s error path sleeps `RETRY` too
    `const RETRY: Duration = Duration::from_millis(100)` is the backoff after a failed poll or accept: a listener with a connection it cannot accept stays readable, so an unbounded poll would spin. A signal is now found at the next wake (a connection, or `stopping`, which the scheduler emits within its 1 s slice), not within 100 ms. The connection that wakes it is admitted, as one in today's 100 ms window was (H8: "A signal still reaches it the way it does now").
  - **`Connection::new`:** `set_nonblocking(true)`, with no read or write timeout.
  - **`run`'s loop, in this order:**
    1. Read `stop`; `flush`; return `"the daemon stopped"` if it was set. This is unchanged, so whatever was queued before `shutdown` is written (Decision 9).
    2. Before a subscribe: if `limits.idle - heard.elapsed()` is zero, return `"it was idle"`; else that remainder is the poll timeout. After a subscribe there is no timeout.
    3. Poll the socket, `stop_wake` and, once subscribed, the subscriber's wake, all `POLLIN`. An unused slot may carry fd `-1`, which `poll` ignores. `Interrupted` loops again; any other poll error returns `"its socket could not be polled"`.
    4. If the subscriber's wake is ready, `drain` it. The drain must come before the next `flush` (step 1 of the next pass): an event queued after the drain then leaves a byte behind, so no wakeup is lost.
    5. If the socket is ready, `read` as now. `WouldBlock` and `Interrupted` read nothing, and `TimedOut` leaves the list.
  - **Subscribe arm:** `Wake::new()`, mapped to `Err("its wake pipe could not be made")` (closes the connection, as a failed `try_clone` does); then `hub.subscribe(socket, Arc::clone(&wake))`; store `(id, events, wake)`.
  - **`flush` and `send`:** both go through `write_all`.
  - **`poll`:** replaces `pending`, with the existing `unsafe` call generalised to a slice. SAFETY comment: "`fds` is a writable slice of `pollfd`s, and the callers keep each descriptor in it open for the call; a negative one is ignored".
  - **Docs:**
    - module doc: the threads block in `poll` on their descriptors and wake pipes (H8, superseding Decision 2's 100 ms read)
    - `accept_loop`: it returns when `stopping` or `shutdown` wakes it, or at a wake after a signal
    - `Limits::write_timeout`: how long a write waits for room
    - `Connection::new`'s comment: the socket is non-blocking
  - **Descriptors:** two per subscriber, plus the two latch pipes (four).

- [ ] **Step 4: Run them to verify they pass**

  Outside the sandbox:
  - `cargo test -p tagteam-daemon --test server --test client --test umask` → PASS: server 22/22 (19 + 3), client 16/16, umask 1/1.
  - `cargo test -p tagteam --features test-support --test daemon_procs` → 9/9. It runs the real daemon over its socket, the signal path through `stopping` included.

  Inside the sandbox: `cargo test -p tagteam-daemon --lib` → PASS.

  No existing assertion changes. `a_stalled_subscriber_is_dropped_alone_and_the_rest_see_every_event` (Review Focus 2) passes unchanged, its `in_emit < 1 s` included. `a_signal_stops_new_connections_but_subscribers_still_hear_stopping` and `tests/client.rs`'s `a_socket_without_a_listener_is_no_daemon` pass unchanged too: their `wait_until` loop's first connect wakes the accept thread, is admitted, and the next is refused. Only the server test's comment (:564-566) changes, to say that the accept thread goes at its first wake after the signal (here that first connect), so connects after it are refused. A prototype of this design passed all of the above on macOS, ten consecutive server runs included, with clippy `-D warnings` clean.

- [ ] **Step 5: Commit**

  `crates/tagteam-daemon/src/server.rs`, `crates/tagteam-daemon/tests/server.rs`: "Wake the socket threads on their descriptors instead of polling every 100 ms".

---

### Task 12: A 12-month report bench, and rows without owned strings (H10.1)

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-engine/tests/tokens_bench.rs`. Update the module doc (:1-7). Update the imports (:18-23). Add constants after `MONTH_RESPONSES` (:31-32). Add `a_year_of_responses` and `peak_rss`, and the new test after the 30-day case (:292-351).
- Modify: `crates/tagteam-engine/src/store/tokens.rs`. Add the new read types after `UsageRow` (:68-81). Add three SQL constants beside `USAGE_COLUMNS` (:331-332). `token_usage` (:878-910) shares their clauses. Add `token_report_usage` after it, and a `#[cfg(test)] mod tests` at the end of the file (:1226).
- Modify: `crates/tagteam-engine/src/store/mod.rs` (:21-24, the `pub use tokens::{…}` list: add the three types).
- Modify: `crates/tagteam-engine/src/tokens/view/group.rs`:
  - the import (:21);
  - `usage` (:36-49) and `Reads` (:51-62);
  - the first pass of `group` (:74-106) and its cost line (:139);
  - `Models` (:413-447) and `Sessions` (:449-481);
  - `series_blocks` (:499) and `Acc::add` (:638).
- Test: `crates/tagteam-engine/tests/store_tokens.rs` (new test), `crates/tagteam-engine/tests/tokens_view.rs` (new pin), and the unit test module in `store/tokens.rs`.
- Not touched: `tokens/view/mod.rs`. `token_report` still calls `group::usage` and builds `group::Reads` the same way, and both types live in `group.rs`. The read order that :364-371 and :402-405 describe still holds, because the usage read is still one statement, run before the pending files and the launches. `TokenReport` is unchanged, so the CLI, its JSON/CSV and `crates/tagteam-tui` are untouched.

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces (in `tagteam_engine::store`):
  - `pub struct ReportResponse { pub session: u32, pub model: u32, pub speed: Option<u32>, pub at_ms: i64, pub counts: TokenCounts, pub finalized: bool }`. It derives `Debug, Clone, Copy, PartialEq`. The three ids are indices into `ReportUsage`'s tables.
  - `pub struct ReportSession { pub session_id: String, pub project: Option<String> }`. It derives `Debug, Clone, PartialEq, Eq`.
  - `pub struct ReportUsage { pub rows: Vec<ReportResponse>, pub sessions: Vec<ReportSession>, pub models: Vec<String>, pub speeds: Vec<String> }`. It derives `Debug, Clone, Default, PartialEq`. Each table lists its entries once, in the order the rows first meet them.
  - `Store::token_report_usage(&self, provider: &ProviderId, since_ms: Option<i64>, until_ms: Option<i64>, session: Option<&str>) -> Result<ReportUsage, StoreError>`. It reads the same rows, in the same order, as `token_usage` with the same arguments.
  - In `group.rs`: `pub(super) fn usage(q: &TokenQuery, store: &Store) -> Result<ReportUsage, StoreError>`, with `Reads.usage: ReportUsage`. Task 13 edits `short_resets` in the same file.

**Decisions.**
- **`UsageRow` stays.** It is the indexer's write type. `refresh.rs` builds it from transcripts before any id exists, and `merge_line` and the batch upsert key on its strings. It also stays the type `token_usage` returns to the tests. The report reads the new types instead, so the strings live once per session, model and speed rather than once per row.
- **The range read follows the time index (`INDEXED BY token_usage_by_at`).** On the bundled SQLite (3.53.2), today's range query has this plan:
  - `SEARCH s USING INDEX sqlite_autoindex_token_sessions_1 (provider=?)`
  - `SEARCH u USING INDEX token_usage_by_session (…)`
  - `USE TEMP B-TREE FOR ORDER BY`

  That sorts every row in the range. A prototype of this task measured three 12-month `month` reports, as medians:

  | Code | Median |
  |---|---|
  | Today | about 2.0 s |
  | Compact rows, today's plan | 1.38 s |
  | Compact rows with the time-index hint | 0.75 s |

  The time index yields `ORDER BY u.at, u.id` without a sort, so the order is unchanged. `token_span` already uses the same hint. The session read already has no sort (it walks `token_usage_by_session`) and keeps its plan.
- **One statement, joins kept.** The joins to `token_sessions` and `token_models` stay. The read selects the integer ids beside the strings and copies a string only the first time its id appears. One statement means one snapshot with no transaction, and the provider filter stays a join, as it is today.
  - Rejected: drop the joins, filter `u.session IN (sessions of the provider)`, and resolve both tables in two more statements inside a read transaction. The prototype measured that about 10% faster, but it adds a transaction and two statements.
- **Prices are still looked up by name per row**, through the table's `&str`. Resolving each model's price once would change `tagteam-core`, and the budget does not need it.
- **Rejected as larger: streaming rows into `group`.** The spec rules it out (mutex re-entrancy, and the snapshot order of token §7.2).
- **How the 12-month case builds its data.** It seeds the store directly through `commit_token_batch`, as `tokens_view.rs`'s `View::index` does. Indexing a year of transcripts instead would mean about 2 GB of files, and this case measures the report, not the index.
  - Each session's responses fall within 4 hours of its start, and a session's rows are committed together. That is the id layout a real index has: a cold index reads one transcript at a time, and incremental refreshes index responses minutes after they happen.
  - The 30-day case spreads each transcript over the whole range. That scatters a time range's rows across the table's pages, a layout no real index has: the prototype read 1.5M such rows in 2.1 s along the time index.
  - The 30-day case itself stays as it is, and with the hint it measured 130–140 ms, against its 200 ms budget.

- [ ] **Step 1: Write the failing tests**

  1. In `tests/tokens_bench.rs`:
     - Imports: add `use tagteam_core::tokens::TokenCounts;`. Extend the `store` import with `UsageRow, usage_key` and the `tokens` import with `default_since_ms`.
     - Module doc: after the 30-day budget, add "and a 12-month `month` report at the same rate, 1 s (review-hardening spec H10.1), whose peak resident memory it prints".
     - Peak memory is measured inside the process, because `/usr/bin/time -l` fails inside the sandbox (`sysctl kern.clockrate: Operation not permitted`) and would also count the seeding. It is recorded, never asserted.

     ```rust
     /// Review-hardening H10.1: a 12-month report (the `month` default, token §11.1) in 1 s.
     const YEAR_QUERY: Duration = Duration::from_millis(1_000);
     /// Twelve months at spec §4's rate: 1,524,000 responses.
     const YEAR_RESPONSES: u64 = 12 * MONTH_RESPONSES;
     /// A thousand sessions a month, as the 30-day case's thousand transcripts.
     const YEAR_SESSIONS: u64 = 12_000;
     /// How long a generated session runs.
     const SESSION_SPAN_MS: i64 = 4 * 3_600_000;

     /// The process's peak resident set in bytes (`ru_maxrss` is bytes on macOS, KiB on Linux).
     fn peak_rss() -> u64 {
         // SAFETY: getrusage writes only into the zeroed struct it is given.
         let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
         assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
         let n = u64::try_from(usage.ru_maxrss).unwrap_or(0);
         if cfg!(target_os = "macos") { n } else { n * 1024 }
     }
     ```

     `a_year_of_responses(fx: &Fx, since_ms: i64, now_ms: i64) -> u64` returns the count it seeded. It works as follows:
     - Start from `Rng(0x9e37_79b9_7f4a_7c15)`. Take `generation = store.begin_token_refresh(&provider, now_ms)`.
     - For each `file in 0..YEAR_SESSIONS`, pick `start` uniformly in `[since_ms, now_ms - SESSION_SPAN_MS)`. Then draw `YEAR_RESPONSES / YEAR_SESSIONS` times uniformly in `[start, start + SESSION_SPAN_MS)` and sort them.
     - Each time `n` becomes one `UsageRow`:

       | Field | Value |
       |---|---|
       | `key` | `usage_key(&provider, &format!("msg_{file}_{n}"), &format!("req_{file}_{n}"))` |
       | `session_id` | `session_of(file)` |
       | `project` | `Some(format!("/work/p{}", file / 20))` |
       | `model` | `MODELS[rng.below(4)]` |
       | `sidechain` | `file % 10 == 9` |
       | `finalized` | `file % 10 != 9` |
       | `counts` | input `below(5_000)`, output `40 + below(400)`, `cache_write_5m` `below(20_000)`, `cache_read` `below(200_000)`, the rest 0 |
       | `speed` | `None` |
     - Commit with `store.commit_token_batch(&provider, generation, &batch, &[], &[], 0, now_ms)` whenever the batch holds at least 10,000 rows, and after the last session. Seeding in batches keeps the seeding's own memory small.

     ```rust
     #[test]
     #[ignore = "timing: run with --release on an idle machine"]
     fn a_12_month_report_query_is_within_1_s() {
         let _one_at_a_time = serial();
         let fx = Fx::new();
         let now = fx.engine.now_ms();
         let since = default_since_ms(&TokenPeriod::Month, now, &jiff::tz::TimeZone::UTC);
         let responses = a_year_of_responses(&fx, since, now);
         assert_eq!(responses, YEAR_RESPONSES);
         // As the 30-day case: one session in ten sighted in a profile over the whole range.
         // (store.record_sighting of session_of(file) for file in (0..YEAR_SESSIONS).step_by(10),
         // home "0192-bench-profile", started_at = first_at = since, last_at = now.)
         let query = || TokenQuery {
             provider: fx.provider(),
             period: TokenPeriod::Month,
             grouping: TokenGrouping::Account,
             since_ms: Some(since),
             until_ms: None,
             accounts: None,
             models: None,
             session: None,
             tz: jiff::tz::TimeZone::UTC,
         };
         let before = peak_rss();
         let report = fx.engine.token_report(&query()).unwrap();
         let after = peak_rss();
         assert_eq!(report.total.responses, responses);
         let runs: Vec<Duration> = (0..5)
             .map(|_| {
                 let started = Instant::now();
                 fx.engine.token_report(&query()).unwrap();
                 started.elapsed()
             })
             .collect();
         let took = median(runs.clone());
         eprintln!(
             "12-month month report: median {took:?} over {responses} responses, {} rows: \
              {runs:?}; peak RSS {:.0} MiB before the first report, {:.0} MiB after",
             report.rows.len(),
             mib(before),
             mib(after)
         );
         assert!(took <= YEAR_QUERY, "a 12-month month report took {took:?}");
     }
     ```

  2. In `tests/store_tokens.rs`, add `ReportUsage, ReportSession` to the `store` import, then:

     ```rust
     /// What a report reads of a response, with `token_usage`'s strings.
     type Read = (String, Option<String>, i64, String, TokenCounts, Option<String>, bool);

     fn as_read(rows: &[UsageRow]) -> Vec<Read> {
         rows.iter()
             .map(|u| (u.session_id.clone(), u.project.clone(), u.at_ms, u.model.clone(),
                       u.counts, u.speed.clone(), u.finalized))
             .collect()
     }

     /// `r`'s rows with their ids resolved through its tables.
     fn resolved(r: &ReportUsage) -> Vec<Read> {
         r.rows
             .iter()
             .map(|e| {
                 let s = &r.sessions[e.session as usize];
                 (s.session_id.clone(), s.project.clone(), e.at_ms,
                  r.models[e.model as usize].clone(), e.counts,
                  e.speed.map(|i| r.speeds[i as usize].clone()), e.finalized)
             })
             .collect()
     }

     #[test]
     fn a_report_read_names_each_session_model_and_speed_once() {
         let (_d, s, _) = store();
         let mut first = row(1, "s1", T, 1);
         first.model = "m-b".into();
         let mut second = row(2, "s2", T + 10, 2);
         second.model = "m-a".into();
         second.project = None;
         second.speed = Some("fast".into());
         second.finalized = true;
         let mut third = row(3, "s1", T + 20, 3);
         third.model = "m-b".into();
         third.speed = Some("standard".into());
         commit(&s, &[third, first, second], T);
         let mut theirs = row(9, "s9", T + 5, 1);
         theirs.model = "m-z".into();
         s.commit_token_batch(&fake(), generation(&s), &[theirs], &[], &[], 0, T).unwrap();

         let r = s.token_report_usage(&cc(), None, None, None).unwrap();
         assert_eq!(
             r.sessions,
             [
                 ReportSession { session_id: "s1".into(), project: Some("/work/app".into()) },
                 ReportSession { session_id: "s2".into(), project: None },
             ],
             "each session once, in the order its first response is read"
         );
         assert_eq!(r.models, ["m-b", "m-a"]);
         assert_eq!(r.speeds, ["fast", "standard"]);
         for (since, until, session) in [
             (None, None, None),
             (Some(T + 10), None, None),
             (None, Some(T + 10), None),
             (None, None, Some("s1")),
             (Some(T + 10), None, Some("s1")),
             (None, None, Some("nope")),
         ] {
             assert_eq!(
                 resolved(&s.token_report_usage(&cc(), since, until, session).unwrap()),
                 as_read(&s.token_usage(&cc(), since, until, session).unwrap()),
                 "{since:?} {until:?} {session:?}"
             );
         }
     }
     ```

  3. In `src/store/tokens.rs`, add a new `#[cfg(test)] mod tests` (`use super::*;`). It holds one helper, `fn plan(sql: &str, p: impl rusqlite::Params) -> Vec<String>`. The helper runs `EXPLAIN QUERY PLAN {sql}` on a fresh `Store::open` in a `tempfile::tempdir()` through `store.lock()`, and collects column 3, as `usage.rs`'s test does. The test:

     ```rust
     #[test]
     fn a_report_reads_a_range_along_the_time_index_without_a_sort() {
         let range = plan(&format!("SELECT {REPORT_COLUMNS} {USAGE_IN_RANGE}"),
                          params!["claude-code", 0, i64::MAX]);
         assert_eq!(range.first().map(String::as_str),
                    Some("SEARCH u USING INDEX token_usage_by_at (at>? AND at<?)"), "{range:?}");
         assert!(!range.iter().any(|l| l.contains("TEMP B-TREE")), "{range:?}");
         let one = plan(&format!("SELECT {REPORT_COLUMNS} {USAGE_OF_SESSION}"),
                        params!["claude-code", 0, i64::MAX, "s"]);
         assert!(!one.iter().any(|l| l.contains("TEMP B-TREE")), "{one:?}");
     }
     ```

  4. In `tests/tokens_view.rs`, add a pin for the speed path. It passes today and must still pass afterwards; no existing report test carries a speed.

     ```rust
     #[test]
     fn a_fast_response_of_a_model_without_a_fast_card_is_unpriced() {
         // Spec §8.3: the speed a response was stored with reaches its price; model-a has no
         // fast card, so its fast response has no price while its others do.
         let v = View::new();
         v.sight("s-a", "acct-a", T0, T0 + 3 * D);
         let mut standard = v.usage("s-a", T0 + H, "model-a", 10, 20);
         standard.speed = Some("standard".into());
         let unset = v.usage("s-a", T0 + 2 * H, "model-a", 5, 0);
         let mut fast = v.usage("s-a", T0 + D + H, "model-a", 10, 20);
         fast.speed = Some("fast".into());
         v.index(&[standard, unset, fast]);

         let r = v.report(&v.query(TokenPeriod::Day, TokenGrouping::None));
         assert_eq!(keys(&r), ["day:2026-09-02", "day:2026-09-01"]);
         // 10 × 1,000 + 20 × 2,000 + 5 × 1,000 nano-dollars.
         let priced = Cost { usd_nanos: 55_000, partial: false };
         assert_eq!((r.rows[0].cost, r.rows[1].cost), (None, Some(priced)));
         assert_eq!(r.unpriced_models, ["model-a"]);
     }
     ```

- [ ] **Step 2: Run them to verify they fail**
  - `cargo test --release -p tagteam-engine --test tokens_bench -- --ignored --exact a_12_month_report_query_is_within_1_s --nocapture`
    - Expected: FAIL with `a 12-month month report took 2.0…s`. The prototype measured a 1.4–2.4 s median, with other builds running on the machine. Seeding takes about 20 s.
    - Record the printed median and both peak RSS figures as today's numbers. The prototype printed `19 MiB before, 451 MiB after`.
  - `cargo test -p tagteam-engine --test store_tokens a_report_read`: expect a compile error, ``no method named `token_report_usage` ``, and unresolved `ReportUsage` and `ReportSession`.
  - `cargo test -p tagteam-engine --lib store::tokens`: expect a compile error, ``cannot find value `REPORT_COLUMNS` ``.
  - `cargo test -p tagteam-engine --test tokens_view a_fast_response`: expect PASS (it is a pin).

- [ ] **Step 3: Implement**
  - **`store/tokens.rs`:**
    - Add the three types under Interfaces, with doc comments. On `UsageRow`'s doc, add that it is the indexer's row and that a report reads `ReportUsage`.
    - Add the constants beside `USAGE_COLUMNS`, with exactly this SQL:

      ```rust
      /// A provider's responses in `[?2, ?3)`, by time, then the order they were first indexed
      /// in. The time index gives that order, so nothing is sorted (review-hardening H10.1);
      /// without the hint SQLite starts from the provider's sessions and sorts every row.
      const USAGE_IN_RANGE: &str = "FROM token_usage u INDEXED BY token_usage_by_at \
           JOIN token_sessions s ON s.id = u.session JOIN token_models m ON m.id = u.model \
           WHERE s.provider = ?1 AND u.at >= ?2 AND u.at < ?3 ORDER BY u.at, u.id";
      /// Session `?4`'s responses in `[?2, ?3)`, in the same order, along its session index.
      const USAGE_OF_SESSION: &str = "FROM token_sessions s \
           JOIN token_usage u ON u.session = s.id JOIN token_models m ON m.id = u.model \
           WHERE s.provider = ?1 AND s.session_id = ?4 AND u.at >= ?2 AND u.at < ?3 \
           ORDER BY u.at, u.id";
      /// What a report reads of a response: its session's and model's ids beside their
      /// strings, which are copied only the first time an id is met, and what `group` sums.
      const REPORT_COLUMNS: &str = "u.session, s.session_id, s.project, u.model, m.name, u.at, \
           u.input, u.output, u.cw_5m, u.cw_1h, u.cache_read, u.web_search, u.speed, u.finalized";
      ```
    - `token_usage`: keep its signature. Its two statements become `SELECT {USAGE_COLUMNS} {USAGE_IN_RANGE}` and `SELECT {USAGE_COLUMNS} {USAGE_OF_SESSION}`. Their WHERE and ORDER BY are today's, so the existing `store_tokens.rs` filter tests now pin the shared clauses.
    - `token_report_usage`: the same `since`/`until` defaults (`i64::MIN`/`MAX`), the same choice between the two statements, and one statement under the one lock, iterated with `query()`/`next()`.
      - A row's `u.session` and `u.model` are looked up in two `HashMap<i64, u32>`. On a miss, push `ReportSession { session_id: r.get(1)?, project: r.get(2)? }` (or `r.get(4)?` into `models`) and record the new index.
      - `u.speed` is read borrowed, through `r.get_ref(12)?`. NULL becomes `None`. Text is interned by a linear `position` search of `speeds`, which holds a handful of values. Text that is not valid UTF-8 is a `rusqlite::Error::FromSqlConversionFailure(12, …)`, as `get::<Option<String>>` gives today.
      - Counts go through `from_sql`, as in `usage_from_row`. No row allocates.
  - **`store/mod.rs`:** re-export `ReportResponse, ReportSession, ReportUsage`.
  - **`group.rs`:**
    - The `crate::store` import swaps `UsageRow` for `ReportResponse, ReportSession, ReportUsage`.
    - `usage()` calls `store.token_report_usage` with today's arguments. `Reads.usage: ReportUsage`, documented as "with the tables its ids index".
    - In `group`, `let usage = &reads.usage;` and the loop walks `usage.rows`. After `Evidence::load`, build `Models::new(&usage.models, q.models.as_deref())` and `Sessions::new(&usage.sessions, &mut evidence.sightings)`. A row's model and session indices are `row.model as usize` and `row.session as usize`, the same indices the tables hold.
    - `q.session` is checked through a `Vec<bool>` per entry of `usage.sessions`, computed once. Only a block report's read is unfiltered.
    - The cost line becomes `prices.cost_nanos(&t.models.names[r.model], &row.counts, row.speed.map(|s| usage.speeds[s as usize].as_str()))`.
    - `Models` becomes `struct Models { names, allowed, unpriced }` without a lifetime, built eagerly: `allowed[i]` is today's filter test of `names[i]`. `by_name`, `filter` and `intern` go, and `Tables` keeps `'q` for `Slots` only.
    - `Sessions::new` builds one `Session` per `ReportSession` and takes each one's sightings out of the map. `by_id`, `intern` and `Default` go.
    - Building every entry eagerly changes no output. A table entry is read only through a group key that holds it, `model_ord` ranks names relative to one another, and `unpriced` is set only by a wanted row.
    - `series_blocks` takes `usage: &[ReportResponse]` (pass `&usage.rows`), and `Acc::add` takes `&ReportResponse`.

- [ ] **Step 4: Run them to verify they pass**
  - `cargo test -p tagteam-engine --lib store::tokens`, `cargo test -p tagteam-engine --test store_tokens --test tokens_view --test tokens_refresh --test invariant --test purge`, and `cargo test -p tagteam-engine --lib`: expect PASS.
  - Outputs must be byte-identical, with no existing test edited. These existing tests pin that:
    - `tokens_view.rs`: every test there, which together cover rows, keys, order, totals, `by_basis`, costs, `unpriced_models`, blocks and drill-down;
    - `store_tokens.rs`: `reads_are_per_provider_by_range_and_session` and `a_session_takes_the_first_project_known_within_a_batch_and_across_batches`;
    - `cargo test -p tagteam --features tagteam/test-support --test tokens_cli --test run_tokens`: the table, `--json` and `--csv`, for example `tokens_day_human_json_and_csv`, `month_defaults_to_twelve_calendar_months` and `session_and_block_columns`;
    - `crates/tagteam-tui/tests/source.rs`, which reads `token_report` and its drill-down, and runs unchanged.
  - Then the bench: `cargo test --release -p tagteam-engine --test tokens_bench -- --ignored --nocapture`.
    - The 12-month case should pass at about 0.75 s, peaking near 194 MiB (prototype). The 30-day case must stay within 200 ms (prototype: 130–140 ms).
    - Record the after numbers next to Step 2's in the task report. Task 20 writes the supported budget into token §1.1/§15.
    - **If the 12-month median exceeds 1 s, STOP.** Do not tune further and do not stream rows. Report both measurements to the controller.
  - Lint the release-only bench too: `cargo clippy --release -p tagteam-engine --tests -- -D warnings`.

- [ ] **Step 5: Commit** — `crates/tagteam-engine/tests/tokens_bench.rs`, `crates/tagteam-engine/src/store/tokens.rs`, `crates/tagteam-engine/src/store/mod.rs`, `crates/tagteam-engine/src/tokens/view/group.rs`, `crates/tagteam-engine/tests/store_tokens.rs`, `crates/tagteam-engine/tests/tokens_view.rs`. Message: "Read report rows as ids along the time index, with a 12-month bench".

---

### Task 13: Samples queries on the primary key (H10.2)

**Depends on:** 12

**Files:**
- Modify: `crates/tagteam-engine/src/store/usage.rs`:
  - add `WINDOW_KEYS` in place of `SAMPLES_OF_ALL` (:207-210), which has no caller left;
  - `usage_samples` (:657-689): its `None` branch and doc;
  - add `usage_window_keys` after it;
  - tests module (:807-829): a new pin, and one line added to the existing pin.
- Modify: `crates/tagteam-engine/src/tokens/view/group.rs`: `short_resets` (:539-563 at 5c9a114; Task 12 does not move it).
- Test: `crates/tagteam-engine/tests/store_usage.rs` (a helper and two tests), plus the unit tests above.
- Not touched: `crates/tagteam-engine/src/views.rs`. `history` (:738) keeps calling `store.usage_samples(&row.id, None, since_s)`. Its `None` branch is now exactly the spec's algorithm: list the keys, read each key with `SAMPLES_OF_WINDOW`, and merge in `(fetched_at, window)` order.
  - That keeps one implementation of "every window".
  - The callers of `usage_samples(_, None, _)` keep their call: `history`, and the tests in `store_usage.rs` and `collect.rs`. Those tests pin the merged order.
  - Rejected: inline the merge in `history` and narrow `usage_samples` to one window. That changes seven test call sites for no behavioural gain.

**Interfaces:**
- Consumes: Task 12's `group.rs` (only `short_resets`, which Task 12 leaves alone).
- Produces: `Store::usage_window_keys(&self, id: &AccountId) -> Result<Vec<String>, StoreError>`. It returns every window key with at least one sample of the account, however old, ascending in byte order (SQLite `BINARY`, which matches `str`'s `Ord`).

- [ ] **Step 1: Write the failing tests**
  1. In `tests/store_usage.rs`, add a helper that writes a sample directly, through the file's `raw` connection:

     ```rust
     /// `id`'s sample of `window` fetched at `at`, written directly.
     fn sample(path: &Path, id: &AccountId, window: &str, at: i64) {
         raw(path)
             .execute(
                 "INSERT INTO usage_samples (account_id, window, fetched_at, pct, resets_at) \
                  VALUES (?1, ?2, ?3, 1.0, NULL)",
                 params![id.as_str(), window, at],
             )
             .unwrap();
     }

     /// Passes today: SAMPLES_OF_ALL's order, which the per-window merge must keep.
     #[test]
     fn every_windows_samples_merge_by_time_then_key() {
         let (_d, path, s) = open();
         let a = add(&s, &cc(), "a", "a@x.co", 1);
         let b = add(&s, &cc(), "b", "b@x.co", 2);
         for (window, at) in [("7d", T), ("5h", T), ("spend", T + 300), ("7d_opus", T + 300),
                              ("5h", T + 600), ("7d", T + 600), ("7d", T - 100)] {
             sample(&path, &a, window, at);
         }
         sample(&path, &b, "5h", T + 50);
         let got: Vec<(String, i64)> = s.usage_samples(&a, None, T).unwrap()
             .into_iter().map(|(k, x)| (k, x.fetched_at)).collect();
         assert_eq!(
             got,
             [("5h".to_owned(), T), ("7d".to_owned(), T), ("7d_opus".to_owned(), T + 300),
              ("spend".to_owned(), T + 300), ("5h".to_owned(), T + 600), ("7d".to_owned(), T + 600)],
             "by time, then key; the account's own; none before since"
         );
     }

     #[test]
     fn window_keys_list_every_window_an_account_has_samples_of() {
         let (_d, path, s) = open();
         let a = add(&s, &cc(), "a", "a@x.co", 1);
         let b = add(&s, &cc(), "b", "b@x.co", 2);
         let c = add(&s, &cc(), "c", "c@x.co", 3);
         let r = reserved(&s, &a, T_MS);
         assert!(s.record_usage(&r, &windows(), T, &plan(), 180).unwrap());
         // The latest reading has the short window alone; the others keep their samples.
         assert!(s.record_usage(&r, &windows()[..1], T + 600, &plan(), 180).unwrap());
         sample(&path, &a, "7d_opus", T - 86_400);
         sample(&path, &b, "zz", T);
         assert_eq!(s.usage_window_keys(&a).unwrap(), ["5h", "7d", "7d_opus", "spend"],
                    "every key with a sample, however old, in byte order");
         assert_eq!(s.usage_window_keys(&b).unwrap(), ["zz"]);
         assert!(s.usage_window_keys(&c).unwrap().is_empty());
     }
     ```

  2. In `src/store/usage.rs`'s `mod tests`, add a new pin beside `one_windows_samples_are_read_along_the_primary_key`, using the same setup. On the bundled SQLite 3.53.2, the whole plan of `WINDOW_KEYS` is `CO-ROUTINE keys, SETUP, SCAN CONSTANT ROW, SCALAR SUBQUERY 1, SEARCH usage_samples USING PRIMARY KEY (account_id=?), RECURSIVE STEP, SCAN keys, CORRELATED SCALAR SUBQUERY 3, SEARCH usage_samples USING PRIMARY KEY (account_id=? AND window>?), SCAN keys`. So the test checks only the lines that name the table:

     ```rust
     #[test]
     fn window_keys_are_listed_by_a_skip_scan_of_the_primary_key() {
         // (store, c = store.lock(), EXPLAIN QUERY PLAN {WINDOW_KEYS} with params!["a"], column 3)
         let reads: Vec<&str> = plan.iter().map(String::as_str)
             .filter(|l| l.contains("usage_samples")).collect();
         assert_eq!(
             reads,
             ["SEARCH usage_samples USING PRIMARY KEY (account_id=?)",
              "SEARCH usage_samples USING PRIMARY KEY (account_id=? AND window>?)"],
             "one seek per key, never a scan: {plan:?}"
         );
         assert!(!plan.iter().any(|l| l.contains("TEMP B-TREE")), "{plan:?}");
     }
     ```

     To the existing pin, add only `assert!(plan.contains("SEARCH usage_samples USING PRIMARY KEY (account_id=? AND window=? AND fetched_at>?)"), "{plan}");`. That checks the per-window read is a range on the whole key, which the merge relies on.

- [ ] **Step 2: Run them to verify they fail**
  - `cargo test -p tagteam-engine --test store_usage`: expect a compile error, ``no method named `usage_window_keys` found for struct `Store` ``. Commenting out that test first shows `every_windows_samples_merge_by_time_then_key` PASS on today's `SAMPLES_OF_ALL`.
  - `cargo test -p tagteam-engine --lib store::usage`: expect a compile error, ``cannot find value `WINDOW_KEYS` ``.

- [ ] **Step 3: Implement** (`store/usage.rs`, then `group.rs`)
  - Replace `SAMPLES_OF_ALL` with this SQL, exactly:

    ```rust
    /// An account's window keys, ascending: a skip-scan of the primary key (account, window,
    /// time), one seek per key, so no scan of the account's samples and no sort. It finds every
    /// key with a sample, including windows the latest reading no longer has.
    const WINDOW_KEYS: &str = "WITH RECURSIVE keys(window) AS ( \
         SELECT (SELECT window FROM usage_samples WHERE account_id = ?1 ORDER BY window LIMIT 1) \
         UNION ALL \
         SELECT (SELECT window FROM usage_samples WHERE account_id = ?1 AND window > keys.window \
         ORDER BY window LIMIT 1) FROM keys WHERE keys.window IS NOT NULL) \
         SELECT window FROM keys WHERE window IS NOT NULL";
    ```
  - `usage_window_keys`: run `WINDOW_KEYS` with `[id.as_str()]` and collect the `String`s.
  - `usage_samples`, `None` branch:
    - Inside one `transaction_with_behavior(TransactionBehavior::Deferred)`, list the keys with `WINDOW_KEYS`. Run `SAMPLES_OF_WINDOW` (prepared once) for each key with `since_s`, appending to one `Vec`, then commit. The listing and the reads share one snapshot, as the single statement did.
    - Then `sort_unstable_by` on `(sample.fetched_at, &key)`. The primary key makes every pair distinct, so the order is total and matches `ORDER BY fetched_at, window`.
    - `let c` becomes `let mut c`. The `Some` branch is unchanged.
    - Rewrite the doc. `window = None` reads the keys `usage_window_keys` lists, each along the primary key, merged by time then key in one read transaction. The rest of the doc stays.
  - `short_resets`:
    - Iterate `store.usage_window_keys(id)?`. Keep the per-key cache: `short.entry(key.clone()).or_insert_with_key(|k| p.describe_window(k).is_some_and(|w| w.kind == WindowKind::Short))`, and skip a key it marks not short.
    - For each short key, read `store.usage_samples(id, Some(&key), since_s)?` and push each `resets_at.saturating_mul(1000)`.
    - `sort_unstable` and `dedup` as today, so the resets are the same set and order as before.

- [ ] **Step 4: Run them to verify they pass**
  - `cargo test -p tagteam-engine --lib store::usage` and `cargo test -p tagteam-engine --test store_usage --test collect --test views_usage --test tokens_view`: expect PASS, with no existing test edited.
  - Outputs identical to today's `SAMPLES_OF_ALL` are pinned by these existing tests:
    - `store_usage.rs`: `a_successful_record_writes_the_reading_the_plan_and_samples`, `samples_filter_by_window_and_time`, `an_empty_reading_is_stored_as_no_windows_with_its_fetch_time`, `a_record_after_another_holder_took_the_lease_is_dropped` and `removing_and_re_adding_keeps_the_hours_count`;
    - `collect.rs`: `an_inactive_account_is_fetched_with_its_stored_token_and_recorded` and `a_ticks_collection_plans_for_the_ticks_threshold_not_the_settings`;
    - `views_usage.rs`: every `history_*` test, especially `history_shows_a_sampled_window_the_latest_reading_lacks` and `history_of_an_empty_latest_reading_still_shows_the_relevant_samples`;
    - `cargo test -p tagteam --features tagteam/test-support --test history`;
    - for `short_resets`, `tokens_view.rs`'s `blocks_are_per_account_with_the_active_projection` (a 5h short and a 7d long window: only the short one makes the observed block) and `a_filter_never_changes_a_blocks_burn_window`.
  - Run `rg -n SAMPLES_OF_ALL crates/` and expect no output.

- [ ] **Step 5: Commit** — `crates/tagteam-engine/src/store/usage.rs`, `crates/tagteam-engine/src/tokens/view/group.rs`, `crates/tagteam-engine/tests/store_usage.rs`. Message: "List an account's window keys by a skip-scan and read each along the key".

---

### Task 14: `add-token`: no echo for `-`, a warning for argv (H11.1, H11.2)

**Depends on:** none

**Files:**
- Modify: `crates/tagteam/src/prompt.rs` (`read_terminal_line`, :257-272, reads with the echo off through a new `read_terminal_line_on`; add the argv-warning helper beside it)
- Modify: `crates/tagteam/src/app.rs` (**exactly one line**, :1980, `Some(t) => Ok(t),`; nothing else in the file)
- Modify: `crates/tagteam/src/cli.rs` (:86-87, a doc comment on `AddToken`'s `token` field)
- Test: `crates/tagteam/src/prompt.rs` (`mod tests`, :476, uses its `pty()`, `flags`, `shown`, `set_after` helpers)
- Test: `crates/tagteam/tests/signals.rs` (next to :419; uses `open_pty`, `std_cmd`, `two_fresh_accounts`, `finish`)
- Test: `crates/tagteam/tests/app.rs` (the in-process `H` harness)
- Test: `crates/tagteam/tests/cli.rs` (:193 `add_token_reads_a_line_from_stdin`)

**Interfaces:**
- Consumes: `EchoOff`, `read_secret`, `open_prompt_terminal` (all in `prompt.rs`).
- Produces: `fn read_terminal_line_on(tty: &File, cancel: &Cancel) -> io::Result<Option<String>>` (private; `pub fn read_terminal_line(cancel)` keeps its signature and opens the terminal, then calls it); `pub const ARGV_TOKEN_WARNING: &str`; `pub fn note_argv_token(err: &mut dyn Write, token: String) -> String`.

Text fixed by the spec (verbatim):

```text
ARGV_TOKEN_WARNING = "warning: the token was passed on the command line, where other local users can see it and your shell history keeps it; use `tagteam add-token -` or run `tagteam add-token` for a prompt instead"
```

```rust
// cli.rs, on `AddToken`'s `token` field (the command's own one-line doc stays as it is, so the
// top-level help page does not change):
/// The token. Pass `-` to read it from stdin, or omit it to be prompted without echo; a token
/// given here is visible to other local users and kept in shell history
token: Option<String>,
```

The `app.rs` line becomes `Some(t) => Ok(prompt::note_argv_token(&mut *self.io.err, t)),`. The helper writes `ARGV_TOKEN_WARNING` and a newline to `err` (ignoring a failed write, as `app.rs` does for its other `warning:` lines) and returns `token`. It prints under `--json` too: stderr is not the JSON channel.

- [ ] **Step 1: Write the failing tests**

`prompt::tests` (need a pty; sandbox off):

```rust
#[test]
fn a_terminal_line_is_read_with_the_echo_off_and_ended_with_a_newline_of_our_own() {
    let pty = pty();
    let tty = pty.open();
    let before = flags(&pty.slave);
    let watch = pty.slave.try_clone().unwrap();
    let reader = thread::spawn(move || read_terminal_line_on(&tty, &Cancel::new()));
    // Type only once the read has turned the echo off; typed earlier, the terminal echoes it.
    let deadline = Instant::now() + Duration::from_secs(3);
    while flags(&watch).3 & libc::ECHO != 0 {
        assert!(Instant::now() < deadline, "the echo was never turned off");
        thread::sleep(Duration::from_millis(10));
    }
    (&pty.master).write_all(b"sk-ant-oat01-secret\n").unwrap();
    assert_eq!(reader.join().unwrap().unwrap(), Some("sk-ant-oat01-secret".to_owned()));
    let screen = shown(&pty.master);
    assert!(!String::from_utf8_lossy(&screen).contains("sk-ant"), "echoed: {screen:?}");
    assert!(screen.contains(&b'\n'), "the line was not ended: {screen:?}");
    assert_eq!(flags(&pty.slave), before, "the terminal is as it was");
}

#[test]
fn a_signal_ends_a_terminal_line_read_with_the_terminal_restored_and_the_line_ended() {
    let pty = pty();
    let tty = pty.open();
    let before = flags(&pty.slave);
    let cancel = Cancel::new();
    let setter = set_after(&cancel, Duration::from_millis(150), libc::SIGINT);
    assert_eq!(read_terminal_line_on(&tty, &cancel).unwrap(), None);
    setter.join().unwrap();
    assert_eq!(flags(&pty.slave), before);
    assert!(shown(&pty.master).contains(&b'\n'));
}
```

`tests/signals.rs`, beside `ctrl_c_while_add_token_reads_a_terminal_stdin_...`:
`add_token_dash_on_a_terminal_does_not_echo_the_token`. Setup as the Ctrl-C test (`two_fresh_accounts`, `open_pty`, stdin = a dup of the slave, stdout and stderr piped), keeping one more clone of the slave. Poll that clone with `tcgetattr` (up to 5 s) until `ECHO` is clear, then write `sk-ant-api03-typed-secret\n` to the master. After `finish(child, 5s)`: exit code 0; read what the master holds without blocking (set `O_NONBLOCK`, read until `EAGAIN`) and assert it does not contain `typed-secret`; assert the child's stderr does not contain `command line`.

`tests/app.rs` (in-process, no sandbox needed):

```rust
#[test]
fn a_token_on_the_command_line_warns_on_stderr_and_one_from_the_prompt_does_not() {
    let h = H::new();
    let (code, _, err) = h.run(&["add-token", "sk-ant-api03-key"], &mut Scripted::none());
    assert_eq!((code, err.as_str()), (0, format!("{ARGV_TOKEN_WARNING}\n").as_str()));
    // the prompt path (`Scripted` answers `secret`): stderr stays empty
    let (code, _, err) = h.run(&["add-token", "--position", "2"], &mut Scripted::answering(&["sk-ant-api03-key2"]));
    assert_eq!((code, err.as_str()), (0, ""));
}
```
(Import `ARGV_TOKEN_WARNING` from `tagteam::prompt`; make the `H::run` call whatever `Scripted` needs to look like a terminal, as the neighbouring prompt tests at :1245-1255 do.)

`tests/cli.rs`: extend `add_token_reads_a_line_from_stdin` with `.stderr("")`, and add `add_token_with_the_token_on_argv_prints_the_warning` (`cmd(d).args(["add-token", "sk-ant-api03-key"])`, `.success().stderr(format!("{ARGV_TOKEN_WARNING}\n"))`).

- [ ] **Step 2: Run them to verify they fail**

`cargo test -p tagteam --lib prompt::tests::a_terminal_line` (sandbox off) fails to compile: `cannot find function read_terminal_line_on`. After stubbing the signature only, the echo assertion fails (the token is on the screen). `cargo test -p tagteam --test app a_token_on_the_command_line` and `--test cli add_token_with_the_token_on_argv` fail with an unresolved import of `ARGV_TOKEN_WARNING`.

- [ ] **Step 3: Implement**

- `read_terminal_line_on`: `read_secret(tty, cancel)` (it sets `EchoOff` and restores it on return), then write `b"\n"` to `tty` on every successful outcome, exactly as `TtyPrompter::secret` (:458-470) does, replacing the `ask("\n")` in the `Interrupted` arm. `Line` is `Some`, `End` is `Some(String::new())`, `Interrupted` is `None`, as today. An error from `EchoOff::new` (not a terminal) propagates as it already does for `open_prompt_terminal`. `tui_host.rs:242` (`wait_for_enter`) is the only other caller and ignores the result; it needs no change, and it now writes the Enter's newline itself instead of getting the terminal's echo.
- The helper, the constant and the doc comment as above. Update the doc comment of `read_terminal_line` ("read as a prompt reads one" now also means "with the echo off").
- Design note for the controller: the warning sits in `prompt.rs` only because `app.rs` may take one line. If the refactor branch later moves `token()`, the helper moves with it.

- [ ] **Step 4: Run them to verify they pass**

`cargo test -p tagteam --lib prompt::tests` and `cargo test -p tagteam --test signals` (both sandbox off: `openpty`), then `cargo test -p tagteam --test app --test cli --test completions --test logging --test home_cli`. Existing tests that pin the old stderr and must change, both in `tests/app.rs`: `declining_add_token_over_an_occupied_position_cancels_and_keeps_the_occupant` (:678) now expects `format!("{ARGV_TOKEN_WARNING}\ntagteam: cancelled\n")`, and the SIGTERM case in `ctrl_c_at_a_choice_or_a_replacement_question_interrupts` (:1299) expects the warning before `INTERRUPTED`. `tests/signals.rs::ctrl_c_while_add_token_reads_a_terminal_stdin_exits_130_without_waiting_for_enter` (:419) must pass unmodified. If a completion-script test pins the positional's help text, update only that string.

- [ ] **Step 5: Commit** — `prompt.rs`, `app.rs` (one line), `cli.rs`, the four test files. "Read add-token's terminal line with the echo off and warn when the token is on argv".

---

### Task 15: Control characters out of human output (H11.4)

**Depends on:** none

**Files:**
- Create: `crates/tagteam-core/src/text.rs`
- Modify: `crates/tagteam-core/src/lib.rs` (`pub mod text;` and `pub use text::printable;`)
- Modify: `crates/tagteam/src/tokens_cmd.rs` (:621, the session PROJECT cell in `human`)
- Modify: `crates/tagteam/src/statusline.rs` (:59, `line`'s `Unmanaged` arm)
- Modify: `crates/tagteam/src/render.rs` (:431, `status_human`'s `Unmanaged` arm; and :456-458, the session's login in the same function)
- Modify: `crates/tagteam-cc/src/oauth.rs` (:122-128, `refusal_message`)
- Test: `crates/tagteam-core/src/text.rs` (`mod tests`), `crates/tagteam/src/tokens_cmd.rs` (`mod tests`, :802; `report_with` at :992), `crates/tagteam/src/statusline.rs` (`mod tests`, :391), `crates/tagteam/src/render.rs` (`mod tests`, :740), `crates/tagteam-cc/tests/oauth.rs` (`both_client_refusal_shapes_are_systemic_and_quote_the_server`, :383, has the `systemic` closure)

**Interfaces:**
- Produces: `pub fn printable(s: &str) -> std::borrow::Cow<'_, str>` in `tagteam_core::text`, re-exported as `tagteam_core::printable`. It returns `Borrowed` when nothing changes. Every C0 character except tab (U+0000-U+001F minus U+0009, newline and ESC included), DEL (U+007F) and every C1 character (U+0080-U+009F) becomes one U+FFFD per character (not per byte). Everything else, U+00A0 and other non-ASCII included, is kept.

- [ ] **Step 1: Write the failing tests**

`tagteam-core` (`text.rs`):

```rust
#[test] fn ordinary_text_and_tabs_come_back_borrowed() {
    for s in ["", "work@x.co", "a\tb", "é — 日本語 😀", "\u{a0}"] {
        assert!(matches!(printable(s), Cow::Borrowed(b) if b == s), "{s:?}");
    }
}
#[test] fn c0_del_and_c1_become_one_replacement_character_each() {
    assert_eq!(printable("a\u{1b}[31mb"), "a\u{fffd}[31mb");
    assert_eq!(printable("x\u{0}\n\r\u{7}\u{7f}y"), "x\u{fffd}\u{fffd}\u{fffd}\u{fffd}\u{fffd}y");
    assert_eq!(printable("\u{80}\u{9b}\u{9f}"), "\u{fffd}\u{fffd}\u{fffd}"); // CSI is two bytes, one character
}
```

One test per site, each with an ESC-bearing value and the exact expected string:
- `tokens_cmd`: `a_session_rows_project_is_printable_in_the_table_and_raw_in_json_and_csv`. A one-row `TokenReport` (from `report_with`, period `TokenPeriod::Session`, one `TokenRow` whose `PeriodKey::Session.project` is `Some("/tmp/\u{1b}[2J\u{7}evil")`, keyed with `RowKey::new`). `human(&report)` contains `/tmp/\u{fffd}[2J\u{fffd}evil` and no `\u{1b}`; `row_json(&row)["period"]["project"]` and the CSV renderer's cell (:506) still hold the raw string.
- `statusline`: `an_unmanaged_email_is_printable`: `line(&StatuslineView::Unmanaged { email: "a\u{1b}[31m@x.co".into() }, DEFAULT_STATUSLINE_FORMAT, NOW, false) == "a\u{fffd}[31m@x.co\n"`.
- `render`: `an_unmanaged_login_is_printable_in_status`: `status_human(&StatusView::Unmanaged { email: "a\u{1b}[31m@x.co".into() }, &ShellAccount::NotInShell, None, NOW, false) == "Live: a\u{fffd}[31m@x.co (not managed by tagteam)\n"`; and with `&ShellAccount::Unmanaged` and `Some("b\u{9b}@x.co")` the last line is `This session: b\u{fffd}@x.co (not managed by tagteam)`. `status_json` is unchanged (assert the raw email in its `Unmanaged` object).
- `oauth` (extends the `systemic` closure's test): `systemic(400, json!({"error":"invalid_client","error_description":"bad\u{1b}[2J client\n"})) == "bad\u{fffd}[2J client\u{fffd}"`, and the nested shape (`{"error":{"type":"invalid_request_error","message":"Client\u{7} with id x"}}`, status 400) gives `"Client\u{fffd} with id x"`.

- [ ] **Step 2: Run them to verify they fail**

`cargo test -p tagteam-core --lib text` (compile error: `cannot find function printable`); after the module exists, `cargo test -p tagteam --lib -- printable unmanaged session_rows` and `cargo test -p tagteam-cc --test oauth both_client_refusal` fail on the unsanitized strings.

- [ ] **Step 3: Implement**

`printable` in the new `text.rs`; wrap the four renderings (`project.clone().unwrap_or_else(..)` is wrapped as `printable(p).into_owned()` for the `Some` case only, not `MISSING`), `format!("{email}\n")` and the two `render.rs` interpolations with it; in `refusal_message` sanitise the chosen string before `to_owned()` (the fallback is a fixed code and needs none). `--json`, CSV, `status_json` and the TUI are untouched. Discrepancy to report: the spec lists `render.rs:431` only, but `status_human` interpolates the run shell's unmanaged `session_login` (:456-458) the same way; this task covers it.

- [ ] **Step 4: Run them to verify they pass**

`cargo test -p tagteam-core -p tagteam-cc --test oauth` and `cargo test -p tagteam --lib`; then `cargo test -p tagteam --test tokens_cli` (human table snapshots must be unchanged: no fixture has control characters).

- [ ] **Step 5: Commit** — all files above. "Replace control characters in the session, email and token-endpoint text people see".

---

### Task 16: The socket client checks the server's uid (H11.5)

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-daemon/src/client.rs` (`connect_at`, :60-69; a pure function and its use)
- Test: `crates/tagteam-daemon/src/client.rs` (`mod tests`, :328), `crates/tagteam-daemon/tests/client.rs` (existing, unchanged, must pass)

**Interfaces:**
- Consumes: `crate::server::peer_uid(&UnixStream) -> io::Result<u32>` (pub, `server.rs:336-373`; on a connected client socket it returns the listener's credentials: `getpeereid` on macOS, `SO_PEERCRED` on Linux).
- Produces: `fn serves_us(peer: u32, own: u32) -> bool` (private, `peer == own`).

Decision for the controller: `server.rs` has a private `own_uid()` (`geteuid`), but Tasks 10 and 11 edit `server.rs`, so this task does not touch it; `connect_at` calls `libc::geteuid()` itself (`libc` is already a dependency of the crate; a `// SAFETY:` comment as `server.rs:325` has). A failure of `peer_uid` itself is treated as a mismatch (fail closed): `NoDaemon` with the same WARN naming the error. The spec is silent on that case.

Log text (WARN, no path, no uid of a person beyond the two numbers):

```text
"the daemon socket is served by another user, so it is not used: server uid {peer}, ours {own}"
```
and for a failed lookup: `"could not tell who serves the daemon socket, so it is not used: {error}"`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn only_a_server_with_our_uid_is_ours() {
    assert!(serves_us(501, 501));
    assert!(!serves_us(0, 501));
    assert!(!serves_us(502, 501));
}
#[test]
fn the_uid_of_a_peer_in_this_process_is_ours() {
    // A uid mismatch cannot be made without root; this pins the plumbing instead.
    let (a, _b) = UnixStream::pair().unwrap();
    assert!(serves_us(crate::server::peer_uid(&a).unwrap(), unsafe { libc::geteuid() }));
}
```

- [ ] **Step 2: Run them to verify they fail**

`cargo test -p tagteam-daemon --lib client::tests` fails to compile: `cannot find function serves_us`. (`UnixStream::pair` works in the sandbox, as the existing `a_connection_closed_before_the_timeouts_are_set_is_no_daemon` shows.)

- [ ] **Step 3: Implement**

In `connect_at`, after the `UnixStream::connect` succeeds and before `greet(stream)` (so nothing is sent and the greeting is not read from a stranger): `peer_uid(&stream)`; `Ok(uid)` with `serves_us(uid, own)` goes on; a mismatch or an `Err` logs the WARN and returns `Ok(Connect::NoDaemon)`. Update `connect_at`'s doc comment (`NoDaemon` also covers a socket served by another user) and `Connect::NoDaemon`'s.

- [ ] **Step 4: Run them to verify they pass**

`cargo test -p tagteam-daemon --lib client::tests` (passes in the sandbox), then, **outside the sandbox** (`bind`): `cargo test -p tagteam-daemon --test client` and `--test server`, both unmodified and passing (every connection there is to a same-uid server).

- [ ] **Step 5: Commit** — `crates/tagteam-daemon/src/client.rs`. "Refuse a daemon socket served by another user".

---

### Task 17: WARN on `primaryApiKey` in a readable `~/.claude.json` (H11.3)

**Depends on:** 5 (Task 5 also edits `live.rs`; this one applies on top)

**Files:**
- Modify: `crates/tagteam-cc/src/config.rs` (a new `pub fn group_or_other_mode(path: &Path) -> Option<u32>` beside `splice_key`; this crate's tests live in `tests/`, so the predicate is public)
- Modify: `crates/tagteam-cc/src/live.rs` (`write_managed_key`, the `in_primary` closure at :939-946: call the check and log, then the existing `splice_key`)
- Test: `crates/tagteam-cc/tests/live_store.rs` (the primaryApiKey tests around :585-640 and the Linux one near :676; `config::` is already imported there)

**Interfaces:**
- Produces: `config::group_or_other_mode(path: &Path) -> Option<u32>`: resolves `path` with `tagteam_provider::atomic::resolve_target` (as the writer does, so a symlinked `~/.claude.json` is judged by its target), `fs::metadata`, and returns `Some(mode & 0o777)` when `mode & 0o077 != 0`; `None` for 0600-or-tighter, an absent or dangling target, or any error.

WARN text (verbatim; it names no path, because §14.2 keeps paths in the user's home out of the log, and no email or key):

```text
"primaryApiKey is going into a Claude Code config file that other users can read; run `chmod 600 ~/.claude.json` (the file in CLAUDE_CONFIG_DIR if that is set) to close it"
```
with the field `mode = %format_args!("{mode:04o}")`.

No refusal and no chmod: the splice proceeds exactly as before (refusing would break API-key accounts on Linux; chmod would change Claude Code's file, core §3, §9.5).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_config_file_others_can_read_is_reported_with_its_mode_and_a_tight_one_is_not() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join(".claude.json");
    for (mode, expected) in [(0o644, Some(0o644)), (0o640, Some(0o640)), (0o604, Some(0o604)), (0o600, None), (0o400, None)] {
        fs::write(&f, "{}").unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(config::group_or_other_mode(&f), expected, "{mode:o}");
    }
    assert_eq!(config::group_or_other_mode(&d.path().join("absent")), None);
}
#[test]
fn a_symlinked_config_is_judged_by_its_target() { /* link -> 0644 file is Some(0o644); link -> 0600 file is None; dangling link is None */ }
```
and in `live_store.rs`, a Linux `write_managed_key` test with a 0644 `.claude.json`: it succeeds, `primaryApiKey` is present, and the file's mode is still 0644 afterwards (no refusal, no chmod).

Choice recorded: the WARN itself is not captured; `tagteam-cc` has no `tracing-subscriber` dev-dependency and the predicate carries the logic. The one-line log call is covered by review and by the clippy build. If the controller wants the line pinned, add `tracing-subscriber` as a dev-dependency (as `tagteam-engine` has) and copy `tests/common`'s `capture_logs`.

- [ ] **Step 2: Run them to verify they fail**

`cargo test -p tagteam-cc --test live_store group_or_other` fails to compile: `cannot find function group_or_other_mode in module config`.

- [ ] **Step 3: Implement**

The predicate in `config.rs`; in `live.rs`, as the first statement of `in_primary` (so both the Linux path at :947 and the macOS Keychain-refusal fallback at :970 pass through it), `if let Some(mode) = config::group_or_other_mode(&paths.global_config) { tracing::warn!(...) }`.

- [ ] **Step 4: Run them to verify they pass**

`cargo test -p tagteam-cc --test live_store --test provider` (no sandbox restriction applies).

- [ ] **Step 5: Commit** — `config.rs`, `live.rs`, `tests/live_store.rs`. "Warn when primaryApiKey goes into a config file other users can read".

---

### Task 18: The token index logs at INFO only when something changed (H11.7)

**Depends on:** none

**Files:**
- Modify: `crates/tagteam-engine/src/tokens/refresh.rs` (:573-582 the `tracing::info!`; the prune result at :540-545 must be kept)
- Test: `crates/tagteam-engine/src/tokens/refresh.rs` (`mod tests`, :636), `crates/tagteam-engine/tests/tokens_refresh.rs` (uses `common::capture_logs`)

**Interfaces:**
- Produces: `fn level(report: &IndexReport, pruned: u64) -> tracing::Level` (private): `INFO` when `!report.complete`, `files_read > 0`, `responses_added > 0`, `responses_updated > 0`, `pruned > 0` or `evidence_changed > 0`; else `DEBUG`. `pruned` is `PruneOutcome::deleted` from the step-5 prune (`IndexReport` has no such field, and `crates/tagteam-tui/src/screens/tokens/index.rs:442` builds an `IndexReport` literal, so the struct must not grow a field). `skipped_lines` and `took_ms` do not count.

- [ ] **Step 1: Write the failing tests**

Unit, in `refresh.rs`'s `mod tests`:

```rust
#[test]
fn a_pass_that_changed_nothing_is_debug_and_anything_else_is_info() {
    let quiet = IndexReport { complete: true, ..IndexReport::default() };
    assert_eq!(level(&quiet, 0), Level::DEBUG);
    assert_eq!(level(&IndexReport { skipped_lines: 3, took_ms: 40, ..quiet }, 0), Level::DEBUG);
    for changed in [
        IndexReport { complete: false, ..quiet },
        IndexReport { files_read: 1, ..quiet },
        IndexReport { responses_added: 1, ..quiet },
        IndexReport { responses_updated: 1, ..quiet },
        IndexReport { evidence_changed: 1, ..quiet },
    ] {
        assert_eq!(level(&changed, 0), Level::INFO, "{changed:?}");
    }
    assert_eq!(level(&quiet, 2), Level::INFO, "a prune that deleted rows");
}
```

Integration, in `tests/tokens_refresh.rs` (`FakeFx`, `append_transcript`, `response`, `line`, `refresh` are there): `refresh_logs_at_info_only_while_something_changes`: append two responses; `let (_, logs) = capture_logs(|| refresh(&ffx.engine, &p));` expect exactly one line that starts with ` INFO` and contains `refreshed the token index`; a second `capture_logs(|| refresh(..))` with nothing new has no INFO line with that message and exactly one ` DEBUG` line containing it; then append a response and the third pass is INFO again.

- [ ] **Step 2: Run them to verify they fail**

`cargo test -p tagteam-engine --lib tokens::refresh::tests::a_pass_that_changed` fails to compile (`cannot find function level`); `cargo test -p tagteam-engine --test tokens_refresh refresh_logs_at_info` fails on the second pass (an INFO line is found).

- [ ] **Step 3: Implement**

Keep the prune's `deleted` in a local (`0` when the prune did not run because the pass was incomplete). Compute `level(&report, pruned)` after the report is built. `tracing::event!` needs a constant level, so emit the line with an `if level == Level::INFO { tracing::info!(..) } else { tracing::debug!(..) }`, same fields and message in both arms (add `pruned` to the fields).

- [ ] **Step 4: Run them to verify they pass**

`cargo test -p tagteam-engine --lib tokens::refresh` and `cargo test -p tagteam-engine --test tokens_refresh --test log_lines`. If a `log_lines.rs` or CLI logging test counted the INFO line of an unchanged pass, change it to expect DEBUG.

- [ ] **Step 5: Commit** — `refresh.rs`, `tests/tokens_refresh.rs`. "Log the token index refresh at DEBUG when it changed nothing".

---

### Task 19: Test doubles behind a `test-doubles` feature (H7)

**Depends on:** 2, 4, 8 (Task 2 also edits `ci.yml` and `CONTRIBUTING.md`)

**Files:**
- Modify: `crates/tagteam-provider/Cargo.toml` (`[features]`, after `lock-log` at line 19: add `test-doubles = []` with a comment)
- Modify: `crates/tagteam-provider/src/clock.rs` (gate `FakeClock`: struct :20, `impl FakeClock` :23, `impl Clock for FakeClock` :37, and the import at :1)
- Modify: `crates/tagteam-provider/src/keychain.rs` (gate `FakeKeychain` and its private helpers, below)
- Modify: `crates/tagteam-provider/src/http.rs` (gate `RecordedRequest`, `type Reply`, `ScriptedHttp` and their impls, below)
- Modify: `crates/tagteam-provider/src/liveness.rs` (gate `FakeProcess`, `FakeProcessProbe` and impls, below)
- Modify: `crates/tagteam-provider/src/process.rs` (gate `ScriptedSpawner` and impls, below)
- Modify: `crates/tagteam-provider/src/lib.rs` (the re-exports at :46, :51-53, :56, :58, :66)
- Modify: dev-dependency lines in `crates/tagteam/Cargo.toml` (:39), `crates/tagteam-cc/Cargo.toml` (:28), `crates/tagteam-engine/Cargo.toml` (:37), `crates/tagteam-daemon/Cargo.toml` (:30), `crates/tagteam-tui/Cargo.toml` (`[dev-dependencies]` :22), `crates/tagteam-fake/Cargo.toml` (`[dev-dependencies]` :16), `crates/xtask/Cargo.toml` (new `[dev-dependencies]`, before `[lints]` at :24)
- Modify: `.github/workflows/ci.yml` (a new `feature-graph` job; Task 2 has already edited this file)
- Modify: `CONTRIBUTING.md` (one sentence, below)
- Test: no Rust test. The checks are the compile matrix and the `cargo tree` assertion below.

All line numbers are at 5c9a114; Tasks 3, 4 and 8 move them in `process.rs`, `lib.rs` and `keychain.rs`. Find items by name.

**Interfaces:**
- Consumes: nothing by name. Tasks 4 and 8 only matter because they add or edit tests and the fake provider that use these doubles: every use they add must sit inside `#[cfg(test)]` code or a crate whose dev-dependencies carry the feature.
- Produces: the Cargo feature `tagteam-provider/test-doubles`; the doubles `FakeClock`, `FakeKeychain`, `RecordedRequest`, `ScriptedHttp`, `FakeProcess`, `FakeProcessProbe`, `ScriptedSpawner` exist (and are re-exported at the crate root and in their modules) iff `cfg(any(test, feature = "test-doubles"))`.

**What stays ungated:** `Env::for_test` (`xtask/src/compat/guard.rs:77` uses it outside tests), `NoHttp` (the statusline's production `Http`), `ReadCounts`/`ReadTally` (the `Keychain` trait's `read_counts` returns them), `FileKeychain` (its own `file-keychain` feature), `Clock`/`SystemClock`.

**Items to gate.** Each gets `#[cfg(any(test, feature = "test-doubles"))]` immediately above the item (above its `#[derive]`, after its doc comment):

| File | Items (lines at 5c9a114) |
|---|---|
| `clock.rs` | `use std::sync::atomic::{AtomicI64, Ordering};` (:1); `struct FakeClock` (:20-21); `impl FakeClock` (:23); `impl Clock for FakeClock` (:37) |
| `keychain.rs` | imports `std::collections::{BTreeMap, BTreeSet}` (:1), `AtomicBool`/`AtomicUsize` (split out of :3, `AtomicU64` and `Ordering` stay), `Mutex` (split out of :4, `Arc` stays); `type Key` (:203); `struct FakeKeychain` (:207); `fn key` (:221); `fn toggle` (:225); `impl FakeKeychain` (:234); `impl Keychain for FakeKeychain` (:281). `fn locked_read` (:176) is also used by `FileKeychain` (:461): gate it `#[cfg(any(test, feature = "test-doubles", feature = "file-keychain"))]` |
| `http.rs` | imports `HashMap`/`VecDeque` (:5) and `Mutex`/`PoisonError` (:7); `struct RecordedRequest` (:181) and `impl fmt::Debug for RecordedRequest` (:188); `type Reply` (:205); `struct ScriptedHttp` (:211); `impl ScriptedHttp` (:216); `impl Http for ScriptedHttp` (:261). Reword the module doc (:2) so it does not claim `ScriptedHttp` is always there. |
| `liveness.rs` | imports `BTreeMap` (:4) and `Mutex`/`PoisonError` (:8); `struct FakeProcess` (:449); `struct FakeProcessProbe` (:461) and its doc line "Tests only (no feature gate: it touches nothing)" (:459), which must now say it is gated; `impl FakeProcessProbe` (:465); `impl ProcessProbe for FakeProcessProbe` (:486) |
| `process.rs` | imports `VecDeque` (:1), `AtomicUsize`/`Ordering` (:10), `Mutex`/`PoisonError` (:12); `struct ScriptedSpawner` (:511); `impl ScriptedSpawner` (:517); `impl ProcessSpawner for ScriptedSpawner` (:542) |
| `lib.rs` | `FakeClock` (:46), `RecordedRequest` and `ScriptedHttp` (:51-53), `FakeKeychain` (:56), `FakeProcess` and `FakeProcessProbe` (:58), `ScriptedSpawner` (:66): each removed from its brace list and re-exported by a separate gated `pub use` |

The import gates are what the compiler demands when the doubles are absent (`unused import` and `dead_code` under `-D warnings`); the list above is complete for 5c9a114 (verified by compiling `clippy --workspace` with and without `--all-targets` and with `--features tagteam/test-support`). Tasks 3, 4 and 8 may start using one of those imports in production code (`run_bounded` plausibly uses `Mutex` or atomics); where they do, drop that import's gate. Let clippy decide: any `unused import` or `never used` error under the no-feature build names the item still to gate or ungate. Run `cargo fmt --all` last; it reorders the split `pub use` lines.

**Cargo.toml lines.** `tagteam-provider/Cargo.toml`:
```toml
# The in-memory doubles other crates' tests use: FakeClock, FakeKeychain, RecordedRequest,
# ScriptedHttp, FakeProcess, FakeProcessProbe, ScriptedSpawner. Only dev-dependencies may enable
# it: dist builds the whole workspace, so a feature a normal dependency enables ships.
test-doubles = []
```
Dev-dependency feature lists (the existing `tagteam-provider = { workspace = true, features = [...] }` line; add `"test-doubles"`):
- `tagteam`: `["file-keychain", "mock-server", "test-doubles"]`
- `tagteam-cc`: `["file-keychain", "test-doubles"]`
- `tagteam-engine`: `["file-keychain", "mock-server", "lock-log", "test-doubles"]`
- `tagteam-daemon`: `["file-keychain", "mock-server", "test-doubles"]`
- `tagteam-tui`, first line under `[dev-dependencies]`: `tagteam-provider = { workspace = true, features = ["test-doubles"] }` (its tests/source.rs uses six doubles)
- `tagteam-fake`, first line under `[dev-dependencies]`: the same line (tests/network.rs and tests/provider.rs use them)
- `xtask`: a new `[dev-dependencies]` table before `[lints]` with the same line. Its only uses are in `src/compat/daemon.rs`'s `mod tests` (:276+), which is `cfg(test)`. Leave the comment above `[dependencies]` as it is; it is the reason the line goes under dev-dependencies.

**Every use of each double across the workspace (checked).** Outside the provider's own gated items, in `src/` every hit is inside a `#[cfg(test)]` module or a `#[cfg(test)] mod testutil;` file: `tagteam-engine/src/{lazy_http,engine,service,testutil,tokens/refresh}.rs`, `tagteam-daemon/src/{scheduler,watch,auto,service,notify,testutil}.rs`, `tagteam-cc/src/naming.rs:139`, `tagteam/src/{app,auto,daemon_cmd,statusline}.rs`, `xtask/src/compat/daemon.rs`. The rest are `tests/` integration tests in `tagteam`, `tagteam-cc`, `tagteam-engine`, `tagteam-daemon`, `tagteam-fake` and `tagteam-tui`, compiled with the dev-dependencies. `engine.rs:47` is a comment. Non-test uses: none. `tagteam-core` and `tagteam-provider/tests/real_keychain.rs` use none. Nothing would break; the one hazard is the dev-dependency line missing in a crate, which `cargo check --workspace` hides because it unifies features across members (see the per-package loop in Step 4).

- [ ] **Step 1: Write the failing check**
  Add the CI job first (it is the test of the feature graph). In `ci.yml`, after `deny`, a job `feature-graph` with `if: github.event_name != 'schedule'`, `runs-on: ubuntu-latest`, `actions/checkout` (same pin, `persist-credentials: false`), the same `Install toolchain from rust-toolchain.toml` step as `check`, no `Swatinem/rust-cache`, and one step:
  ```yaml
  - name: test-doubles is not in the normal-dependency graph
    run: |
      tree=$(cargo tree --workspace -e normal,features -i tagteam-provider --locked)
      printf '%s\n' "$tree"
      grep -q '^tagteam-provider ' <<<"$tree"
      if grep -F 'feature "test-doubles"' <<<"$tree"; then
        echo "::error::a normal dependency enables tagteam-provider's test-doubles feature"
        exit 1
      fi
  ```
  The `grep -q '^tagteam-provider '` line fails the job if the package is renamed, so the absence check cannot pass vacuously. Add a comment above the job saying why it has no cache (same reason as `deny`).
  Run the same commands locally before any other edit: on 5c9a114 they pass (the feature does not exist yet), so this step cannot fail yet; the failing proof is the negative check in Step 4.
  Then add `test-doubles = []` and the dev-dependency lines only. `cargo check --workspace --all-targets --locked` passes (nothing is gated yet).

- [ ] **Step 2: Run them to verify the failing case**
  Apply the gating of Step 3 first, without the dev-dependency line for `tagteam-tui`. `cargo check -p tagteam-tui --all-targets --locked` then fails: `error[E0432]: unresolved imports tagteam_provider::FakeClock, tagteam_provider::FakeKeychain, tagteam_provider::ScriptedHttp` (plus `liveness::FakeProcessProbe` and `process::ScriptedSpawner`; verified by removing the line in a scratch copy). Adding the line fixes it; the same holds for each of the seven crates.

- [ ] **Step 3: Implement**
  Gate the items in the table, rewrite `lib.rs`'s re-exports, add the dev-dependency lines. Approach: cfg attributes only, no module moves, no signature changes. One sentence in `CONTRIBUTING.md` near line 44: "`tagteam-provider`'s in-memory doubles (`FakeKeychain`, `FakeClock`, `ScriptedHttp`, `FakeProcessProbe`, `ScriptedSpawner`) sit behind its `test-doubles` feature, which only dev-dependencies may enable."

- [ ] **Step 4: Run them to verify they pass**
  - `cargo fmt --all --check`.
  - Without the feature in a normal dependency (the shipped shape): `cargo clippy --workspace --locked -- -D warnings` (libs and bins only). Expected: clean. This is the check that the gates left no dead code or unused import.
  - `cargo clippy --workspace --all-targets --locked -- -D warnings` and the same with `--features tagteam/test-support`. Expected: clean.
  - Per-package, to prove each crate's own dev-dependency line (a workspace-wide run unifies features and hides a missing one): `for p in tagteam tagteam-cc tagteam-engine tagteam-daemon tagteam-tui tagteam-fake xtask tagteam-provider tagteam-core; do cargo check -p $p --all-targets --locked || break; done`. Expected: all pass (verified in a scratch copy at 5c9a114 with this change). Provider with its other features: `cargo clippy -p tagteam-provider --locked --features file-keychain,mock-server,lock-log,real_keychain -- -D warnings`.
  - The CI command: `cargo tree --workspace -e normal,features -i tagteam-provider --locked` prints no line containing `feature "test-doubles"`. Negative proof, in a scratch edit you revert: change `tagteam-fake`'s normal `tagteam-provider.workspace = true` to carry `features = ["test-doubles"]`; the tree then prints `tagteam-provider feature "test-doubles"` and the job's script exits 1.
  - `cargo test --workspace --locked --no-fail-fast --features tagteam/test-support` passes as before. Tests that must run outside the sandbox are the usual ones (`tagteam`'s `prompt::tests`, `tests/signals.rs`, `tests/tui_pty`; daemon `client`/`server` tests).
  - `cargo test --workspace --doc --locked`: no doctest names a gated double (checked by grep; this confirms it).
  - Note for the MR: a binary built with dist's `--workspace` links no double because the items are not compiled; `strings` on the binary proves nothing (unreferenced code is dropped by the linker either way), so the graph assertion is the evidence.

- [ ] **Step 5: Commit** — the seven `Cargo.toml` files, `crates/tagteam-provider/src/{clock,keychain,http,liveness,process,lib}.rs`, `.github/workflows/ci.yml`, `CONTRIBUTING.md`: "Compile the provider's test doubles only for tests and check the feature graph in CI".

---

### Task 20: SECURITY.md and the amended specs (H13)

**Depends on:** all others (the controller applies it last, so the text states what shipped)

**Files:**
- Modify: `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-daemon-design.md`
- Modify: `docs/superpowers/specs/2026-10-14-tagteam-token-report-design.md`
- Modify: `docs/superpowers/specs/2026-10-19-tagteam-review-hardening-design.md` (an `## Errata` section at the end, hunk E1)
- Modify: `SECURITY.md`
- Test: a verification step only (greps below). Nothing under `crates/`.

**Interfaces:** none.

Hunks are small and replace existing text. "Old" is quoted as it stands at 5c9a114 (line numbers from there). H5.1 is conditional on Task 1's gate: if Task 1 failed and H5.1 was dropped, the controller deletes hunk C1's `-w` wording and keeps its argv/WARN sentence. `<budget from Task 12>` in T3 and T4 is for the controller to fill in from Task 12's measurement.

No code is written in this task, so Steps 1-4 collapse to: apply the hunks (Step 3), verify (Step 4), commit (Step 5).

- [ ] **Step 1: Write the failing tests** — run the "must-not-match" greps of Step 4 now and record that each matches (they are the old phrasings).
- [ ] **Step 2: Run them to verify they fail** — each `rg` in Step 4's first block prints at least one line.

- [ ] **Step 3: Implement** — apply every hunk below.

#### Core spec (`2026-09-26-tagteam-core-cli-design.md`)

**C1. Appendix A.3 `security` driver, bullet "Write"** (:3500-3504).
Old:
```text
- **Write:** `security -i`, with the stdin line `add-generic-password -U -a "<acct>" -s "<svc>"
  -X "<hex>"`.
  - If the line exceeds 4032 bytes (the 4096-byte `-i` line limit minus 64), use argv instead.
    An over-long `-i` line truncates silently and leaves the old entry.
```
New:
```text
- **Write:** `security -i`, with the stdin line `add-generic-password -U -a "<acct>" -s "<svc>"
  -w "<text>"` when the credential bytes are UTF-8 with no NUL, CR or LF, `<text>` being the
  credential with `\` written `\\` and `"` written `\"`; any other credential is written as
  `-X "<hex>"`. `security -i` unescapes `\"` and `\\` and passes non-ASCII, `#`, `$`, `%` and
  tab through; JSON costs about 1.1 bytes per byte this way, against 2 for hex.
  - If the chosen line exceeds 4032 bytes (the 4096-byte `-i` line limit minus 64), use argv
    instead, with the same encoding (`-w <text>` or `-X <hex>`), and log a WARN that names the
    service, the account ID and the byte length, never the value. Claude Code does the same.
    An over-long `-i` line truncates silently and leaves the old entry.
```

**C2. Appendix A.3, bullet "Active reads"** (:3533).
Old: `- **Active reads** retry the Keychain twice, 300 ms apart. CC caches Keychain reads for 30 s and`
New: `- **Active reads** retry the Keychain twice, 300 ms apart, except after rc 36 (locked), which will not change within the retry window. CC caches Keychain reads for 30 s and` (the rest of the sentence is unchanged).

**C3. Appendix A.4 Credential shape, bullet "Machine-shared keys"** (:3567-3569): append after the key list:
```text
  tagteam never keeps them in its own copies: the vault and `rescue/` hold the credential
  without them (§6.2), and `--full` exports (§13.3) omit them. `displaced/` files stay verbatim,
  since they keep exactly what was not tagteam's to keep.
```

**C4. §6.2 Vault, bullet "Contents"** (:575-577).
Old: `The bytes are stored raw, in whatever form the provider defines. For CC that is the credential JSON, or an \`sk-ant-api…\` string for API-key accounts. The vault is provider-agnostic: it never parses what it stores.`
New:
```text
The bytes are stored in the provider's vault form, which the provider produces and the vault
  never parses. For CC that is the credential JSON without the five machine-shared keys
  (Appendix A.4), every other key kept in its order, or an `sk-ant-api…` string for API-key
  accounts. A credential that is not a JSON object is stored unchanged. A vault item written
  before this rule keeps its bytes until its next store; nothing migrates it.
```

**C5. §6.2, bullet "Pending rescues before activation"**, first sub-bullet (:605-606).
Old: `- A rescue whose \`predecessorFp\` is the vault's current fingerprint is adopted, as §7.3 step 3 adopts it, and the adopted generation is the one activated.`
New:
```text
  - A rescue whose own fingerprint is the vault's current fingerprint is deleted first (the
    landed rule, §6.3). A rescue whose `predecessorFp` is the vault's current fingerprint is
    adopted, as §7.3 step 3 adopts it, and the adopted generation is the one activated.
```

**C6. §6.3, bullet "`rescue/`"** (:620-622).
Old: `- **\`rescue/\`** holds a refreshed successor whose vault write failed. The next pass of the refresh gate for that account adopts it automatically (§7.3), and the file is deleted only after a verified vault write.`
New:
```text
- **`rescue/`** holds a refreshed successor. It is written ahead of the vault write (§7.3 step
  6, §7.5 step 5) and deleted after a verified vault write, so a file that outlives its refresh
  is a vault write that failed or a process that was killed. The next pass of the refresh gate
  for that account adopts it automatically (§7.3), and the file is deleted only after a
  verified vault write. A successor that belongs to another account is never written ahead; it
  goes to `displaced/` only.
  - **Landed rule.** Before it adopts anything, every code path that settles an account's
    rescues (the gate's pass, activation's settling in §6.2, and §7.5 step 3) deletes a rescue
    whose own (successor) fingerprint equals the vault's current fingerprint. CC's fingerprint
    hashes the refresh token, so the vault then already holds a valid generation with that
    refresh token, and deleting the rescue loses at most an access token that refresh token
    re-obtains. This covers a crash after the vault write landed and before the delete, and a
    same-generation successor (a reply without a new refresh token keeps the fingerprint). A
    reader that only picks a rescue (export, §13.3) skips such a rescue.
```
and in the envelope sub-bullet (:624-625) replace `\`credential\` holds the credential bytes verbatim, as a UTF-8 string.` with `\`credential\` holds the credential bytes in vault form (§6.2), as a UTF-8 string.`

**C7. §7.3 Refresh gate, step 3, second bullet** (:818-820): after "adopt it: write it to the vault, verify, delete the rescue file, and use it." add ` A rescue whose own fingerprint is the vault's current one has landed and is deleted instead (§6.3).` (consistency edit; H13 lists §6.2/§6.3 and this step refers to the same rule).

**C8. §7.3 step 6** (:829-846). Replace the heading line and the first three bullets as follows.
Old heading: `6. **Persist, compare-and-swap style.** Re-read the vault. With every writer under the account lock this comparison cannot fail; it stays as a defence.`
New:
```text
6. **Write ahead, then persist, compare-and-swap style.** In this order:
   1. As soon as a received successor is known to belong to this account, before anything else
      in this step, write its rescue envelope (§6.3, vault form) with the fsynced 0600 writer.
      This is best effort: a failure is logged and the step goes on.
   2. Re-read the vault and compare. With every writer under the account lock this comparison
      cannot fail; it stays as a defence.
   3. Persist to the vault and verify the write.
   4. Only then delete the envelope.
```
Bullet 2 old: `- If the fingerprint moved since step 3, write the new successor to \`rescue/\`, log at ERROR, and return the vault's newer credential.` → new: `- If the fingerprint moved since step 3, keep the written-ahead envelope as the rescue, log at ERROR, and return the vault's newer credential.`
Bullet 4 old: `- If the vault write fails, write to \`rescue/\` and return \`Transient { credential, rescued: true }\`.` → new: `- If the vault write fails, the written-ahead envelope is the rescue (no second copy is written; if the write-ahead itself failed, write it now) and return \`Transient { credential, rescued: true }\`.` (the following sentence "The caller must not activate…" stays).
The sentence under the identity-conflict bullet "never written to the vault or to `rescue/`" already covers the foreign successor.

**C9. §7.5 step 3, before the table** (:911, after "…is never used to decide it."): insert
```text
   The landed rule (§6.3) comes first: a rescue whose own fingerprint equals the vault's
   current fingerprint is deleted before any row below applies, so the rows see no such rescue.
```
and in the table's first row, old `If a rescue succeeds this generation, publish it: vault, then live store` → new `If a rescue that survived the landed rule succeeds this generation, publish it: vault, then live store`.

**C10. §7.5 step 5** (:927). Old: `Persist the successor to the vault first, or to \`rescue/\` if that fails. Then write it to the live store in either case,` → new: `Write the successor's rescue envelope ahead (§7.3 step 6), then persist it to the vault and delete the envelope once that verifies; if the vault write fails the envelope stays as the rescue. Then write it to the live store in either case,`

**C11. §12.5, "Token evidence" paragraph** (:2255). Old: `a scoped thread checks the child's session record every 50 ms, reading it when its mtime changed,` → new: `a scoped thread checks the child's session record every 250 ms, reading it when its mtime changed,`.

**C12. §12.5 "Signals", bullet "While `claude` runs"** (:2268-2270): append `The parent learns of the child's exit at once, from a thread that waits for it without reaping it; it checks the cancel token, and forwards a signal it holds, every 250 ms. Only the parent reaps, so a forwarded signal never reaches a recycled pid.`

**C13. §13.3, "Default contents are slim"** (:2714-2716).
Old: `the machine-shared keys and the device-bound \`trustedDeviceToken\` stay on the source machine. \`--full\` keeps everything.`
New: `the machine-shared keys and the device-bound \`trustedDeviceToken\` stay on the source machine. \`--full\` keeps every account-scoped key (\`trustedDeviceToken\` included) and no machine-shared one, whatever the credential's source (the live login, a session profile or the vault).`

**C14. §14.1 Signals and cancellation.** In "Cancellation points" (:2995-3000) add a bullet after "before reserving or sending a usage request;":
```text
  - a Keychain read inside usage collection that comes before any request (the stored token's
    vault read and the active account's live-credential read). Keychain writes, and every read
    inside a critical span below, are never cancellation points;
```
and in "Critical spans" add to the last bullet: `any vault, rescue or atomic write;` → `any vault, rescue or atomic write, and the Keychain reads they make (a write's verify read, the gate's compare-and-swap read, the switch's and recovery's reads).` Extend "An interrupted usage fetch is not a usage failure" with `, nor is an interrupted Keychain read there`.

#### Daemon spec (`2026-10-14-tagteam-daemon-design.md`)

**D1. D4.6 Signals and stopping** (:271).
Old: `The accept and connection threads poll with 1 s timeouts and stop too.`
New: `The accept and connection threads block on their descriptors and on wake pipes that the stop writes, and stop too. After a signal, the accept thread ends at its next wake: a connection, or the socket's close at \`stopping\`. (This supersedes daemon-b plan Decision 2's 100 ms read timeout; a connection's 30 s pre-subscribe idle timeout and 2 s write timeout stay.)`

#### Hardening spec (`2026-10-19-tagteam-review-hardening-design.md`)

**E1.** Append at the end of the file:
```text
## Errata

Found while planning and building; each is listed in the MR.

- **H1.2:** the branch touches `switch.rs` a third time: one line at the direct branch of
  step 2 (`switch.rs:2043`) compares the live entry and the target's vault bytes both in
  vault form, or `switch --force` onto the live account would displace its own entry (H5.3).
- **H5.1:** the hex line is never shorter than the text line, so `-X` is used only for bytes
  that cannot be text; past the limit, argv carries the encoding the line would have used.
- **H6:** `export.rs:494-550` is the session-owned export path; an idle account's export goes
  through `settle_rescues`. All three paths apply the landed rule.
- **H8:** the wake pipes add up to 36 descriptors, not 32: two shared pipes beside the 16
  connections' own.
- **H10.1:** compact rows alone missed the 1 s budget; the range read also names its index
  (`INDEXED BY token_usage_by_at`), pinned by an EXPLAIN test. <Task 12's measured numbers>
```
The controller fills `<Task 12's measured numbers>` from Task 12 (today's time and peak memory, and after).

**D2. D6.2 Text and delivery**, new first sub-point before "**Each delivery**" (:432):
```text
- **The notifier thread** blocks until a notice arrives or its stop message does; it never
  wakes to check a flag. Dropping the `Notifier` sends the stop message.
```

#### Token report spec (`2026-10-14-tagteam-token-report-design.md`)

**T1. §6.1 `run`, bullet "Poll"** (:510). Old: `on every 50 ms tick of a scoped thread` → new: `on every 250 ms tick of a scoped thread`. (Supersedes M4b plan Decision 1's 50 ms tick for `run`.)

**T2. §7.4 Assumptions, A3** (:675-676). Old: `\`run\` every 50 ms, the scan far less often.` → new: `\`run\` every 250 ms, the scan far less often.` and append to the A3 paragraph: `A tenure ends at its last confirmation, so at most the responses of the last 250 ms before it ends go unsighted; with \`run\`'s launch open they have two candidates and stay unattributed, never given to the wrong account.`

**T3. §1.1 item 3** (:35). Old: `a 30-day report query takes ≤ 200 ms (release build, Apple silicon).` → new: `a 30-day report query takes ≤ 200 ms, and the default 12-month range (about 1.5M responses) takes ≤ <budget from Task 12> for the query and grouping (release build, Apple silicon).`

**T4. §15 Testing, "Budgets"** (:1144-1145). Old: `A benchmark generates a 500 MB synthetic tree and checks the refresh and query budgets of §1.1, scaled.` → new: same sentence plus ` A second case generates 12 months at the spec's rate (about 1.5M responses) and checks the 12-month budget of §1.1 (release builds only).`

#### SECURITY.md

**S1. New section "Verifying a release"**, after "Supported versions":
```text
## Verifying a release

Tagged releases carry GitHub artifact attestations for every release file. To check a file you
downloaded:

    gh attestation verify <file> --repo michaelrigart/tagteam

It confirms that the file was built by this repository's release workflow. A file that fails
the check should not be run; report it as described above.
```

**S2. Advisory job replacing the hand-kept note.** Delete the "Out of scope" bullet (:74-76, starting `- RUSTSEC-2023-0071 (RSA timing side channel)`) and add to "What tagteam stores and protects":
```text
- **Dependencies.** CI runs `cargo deny check advisories` on every push and pull request and
  once a week, and fails on a RustSec advisory against a locked crate (a vulnerability or an
  unsound advisory anywhere in the graph; an unmaintained crate among tagteam's direct
  dependencies). The one advisory it ignores, and each reason, is recorded in `deny.toml`.
```

**S3. H5.2 disclosure**, new bullet after "Credentials live in the vault" (:30):
```text
- **Credentials and `security`'s command line.** tagteam passes a credential to
  `/usr/bin/security` on standard input (`security -i`), not on its command line. When a
  credential exceeds about 3.8 KB, `security` receives it on its command line instead, which
  macOS shows only to the same user and root but which exec-auditing tools record. Claude
  Code's own Keychain writes do the same. tagteam logs a WARN when it happens (the service,
  the account ID and the length, never the value). The vault and `rescue/` copies hold no
  machine-shared key (MCP OAuth tokens, plugin secrets), which keeps nearly every credential
  well below that size.
```

**S4. H6 note**, in the "Files that hold secrets" bullet (:37-45), old `\`rescue/\` (a refreshed token whose vault write failed)` → new `\`rescue/\` (a refreshed token, written before its vault write and kept if that write fails)`, and append: `On macOS that means a refreshed token also sits in a 0600 file for about 100 ms per refresh, in a 0700 directory; it is encrypted at rest when FileVault is on, and a local snapshot taken inside that window keeps a token that the next refresh supersedes.`

**S5. H11.2**, new bullet after "Credentials live…":
```text
- **Passing a token.** `tagteam add-token <TOKEN>` puts the token on the command line, where
  other local users can see it and your shell history keeps it; tagteam prints a warning.
  Use `tagteam add-token -` (it reads standard input, with no echo on a terminal) or
  `tagteam add-token` with no argument (a prompt).
```

**S6. H11.3**, new bullet:
```text
- **`~/.claude.json` and API keys.** On Linux, and on macOS when the Keychain refuses the
  write, an API-key account's key is stored as `primaryApiKey` in `~/.claude.json`, the way
  Claude Code stores it itself. tagteam keeps that file's mode and never changes the mode of a
  Claude Code file. The file is often 0644, which lets other local users read the key. tagteam
  logs a WARN when it writes the key into a file that group or others can read; the fix is
  `chmod 600 ~/.claude.json` (the file in `CLAUDE_CONFIG_DIR` if that is set).
```

**S7. H11.6** (:57-59).
Old: `and never sends a superseded token to the token endpoint. A copy of a login exported to another machine stops working as soon as either machine refreshes it.`
New: `and never sends a generation it knows was superseded to the token endpoint. After an ambiguous failure (a timeout or reset with no response read) it deliberately retries the same generation, because the server may never have processed the request; if it had, the retry gets \`invalid_grant\` and the account is marked for a new login. A copy of a login exported to another machine stops working as soon as either machine refreshes it.`

- [ ] **Step 4: Run them to verify they pass**

Must now print nothing (`S` is `docs/superpowers/specs`):
```text
rg -n 'every 50 ms|50 ms tick' S/2026-09-26-tagteam-core-cli-design.md S/2026-10-14-tagteam-token-report-design.md
rg -n 'poll with 1 s timeouts' S/2026-10-14-tagteam-daemon-design.md
rg -n 'retry the Keychain twice, 300 ms apart\.' S/2026-09-26-tagteam-core-cli-design.md
rg -n 'whose vault write failed\. The next pass|stored raw, in whatever form|keeps everything\.' S/2026-09-26-tagteam-core-cli-design.md
rg -n 'never sends a superseded token|RUSTSEC' SECURITY.md
```
Must each print at least one line (the new phrasings): `rg -n 'gh attestation verify|cargo deny check advisories|chmod 600|add-token -|ambiguous failure|100 ms per refresh' SECURITY.md` (6 distinct matches); `rg -n 'landed rule|Landed rule' S/2026-09-26-tagteam-core-cli-design.md` (§6.2, §6.3, §7.5); `rg -n 'every 250 ms' S/2026-09-26-tagteam-core-cli-design.md S/2026-10-14-tagteam-token-report-design.md`; `rg -n 'wake pipes' S/2026-10-14-tagteam-daemon-design.md`; `rg -n '<budget from Task 12>' S/` finds exactly the two placeholders until the controller fills them, then nothing. Then `git diff --stat` shows only the four files; `cargo fmt --all --check` is unaffected.

- [ ] **Step 5: Commit** — the four files. "Amend the core, daemon and token report specs and SECURITY.md for the review-hardening changes".
