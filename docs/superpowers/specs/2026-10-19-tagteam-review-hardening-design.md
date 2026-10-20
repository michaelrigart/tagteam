# tagteam — review hardening: security and performance

**Status:** Approved
**Date:** 2026-10-19
**Scope:** The security and performance findings of the codebase review of 2026-10-19, run
on `main` at 683b266. The reports are not in the repository; this spec carries everything a
builder needs from them. Every cited path:line was re-verified at 683b266. A reference such as
"§7.3" is to the core spec
([`2026-09-26-tagteam-core-cli-design.md`](2026-09-26-tagteam-core-cli-design.md)); "D4.6" is
to the daemon spec ([`2026-10-14-tagteam-daemon-design.md`](2026-10-14-tagteam-daemon-design.md));
"token §6.4" is to the token report spec
([`2026-10-14-tagteam-token-report-design.md`](2026-10-14-tagteam-token-report-design.md));
"TUI §6.4" is to the TUI spec ([`2026-10-14-tagteam-tui-design.md`](2026-10-14-tagteam-tui-design.md)).
"H3" is to section 3 of this spec.

---

## H1. Purpose

The review found no Critical issue and one High (ARCH-1, owned by the refactor branch). This
branch owns the security and performance axes. It closes the release pipeline's cache
poisoning path, keeps credentials off `/usr/bin/security`'s command line, makes a refreshed
token survive a kill, gives the workspace one subprocess runner, keeps test doubles out of the
release binary, cuts the daemon's and `tagteam run`'s idle wakeups, and bounds the token
report's cost at its default 12-month range.

### H1.1 Success criteria

1. No release build restores an Actions cache. Tagged releases carry GitHub artifact
   attestations. CI fails on a new RustSec advisory against a locked crate.
2. On macOS, a credential whose `security -i` line fits 4032 bytes never appears on any
   process's argv. Vault and `rescue/` copies hold no machine-shared key.
3. Once a successor's write-ahead envelope is durable, a SIGKILL at any later instant leaves
   the successor in the vault or `rescue/`. What stays exposed is the time from the response
   to that one fsynced file write, and the whole vault write when the write-ahead itself
   failed (H6, H15).
4. Every bounded child process (`security` included) is waited for by one implementation,
   which notices the child's exit without a polling sleep and kills its whole process group on
   timeout or cancel.
5. The release `tagteam` binary contains none of `tagteam-provider`'s test doubles.
6. An idle daemon wakes about once a second, whatever number of clients is subscribed.
   `tagteam run` wakes about 8 times a second instead of 40.
7. A 12-month token report over about 1.5M responses stays within the budget H10.1 sets.
8. Behaviour, output and exit codes are otherwise unchanged; existing tests pass unmodified
   except where a section here changes the behaviour they pin.

### H1.2 Non-goals and file ownership

- The refactor branch (`review-refactor`) owns `crates/tagteam-engine/src/switch.rs`,
  `launch.rs`, `doctor.rs` (wherever they live), `crates/tagteam/src/app.rs`, and the
  duplicated switchable-account rule in the daemon (`tagteam-daemon/src/auto.rs`,
  `provider_job.rs`). PERF-R-1 (the switch's spawn count) is that branch's.
- A change here that must touch one of those files is kept minimal and listed in the MR. This
  spec needs exactly two: one line in `app.rs` (H11.2) and, if the builder finds it necessary,
  the argument list of `switch.rs`'s one `vault.store` call (H5.3). This branch merges first;
  refactor rebases onto it.
- A TUI UI/UX review has started and will likely redesign `crates/tagteam-tui`. This branch
  changes nothing there except the dev-dependency line H7 adds to its `Cargo.toml`; the
  TUI-side performance items wait for that review (H12).
- No schema version bump (§6.1's ladder stays at 3).
- No new runtime dependency. `libc` and `signal-hook` already provide what H4, H8 and H9 need.

## H2. Decisions taken with Michael (2026-10-19)

| # | Question | Decision |
|---|---|---|
| 1 | SEC-C-1: keep credentials off `security`'s argv | Strip machine-shared keys from tagteam's own copies, send credentials as escaped `-w` text inside `-i`, and keep argv only as a logged last resort (H5). |
| 2 | PERF-R-3/PERF-R-4: idle wakeups | Daemon threads block on their descriptors and wake pipes; `tagteam run` notices exit at once and ticks at 250 ms (H8, H9). |
| 3 | PERF-D-2: a live session's heartbeat forces a token report re-query | Keep as intended (token §6.4, §10). The TUI re-queries every 60 s anyway (TUI §6.4), so gating saves almost nothing with one live session; H10 makes each re-query cheaper instead. |
| 4 | SEC-C-4: write `rescue/` ahead of the vault | Yes, accepting a short-lived 0600 plaintext copy on macOS (H6). |

## H3. Release pipeline (SEC-S-1, SEC-S-5)

**Finding.** `release.yml:130` restores `swatinem/rust-cache` in `build-local-artifacts`, the job
that builds the shipped binaries. `cc-drift.yml` runs `curl … claude.ai/install.sh | bash` on
the default branch weekly. Code running there can write a cache entry under the release job's
key prefix, and a tag build restores it. The only other user is `ci.yml`'s `check` job, whose
results ship nothing. There are no artifact attestations and no advisory scanning.

**Design.**
- `dist-workspace.toml`: `cache-builds = false`, `github-attestations = true`,
  `github-attestations-phase = "host"`. Pin `actions/attest` by commit SHA in
  `[dist.github-action-commits]`, as the other actions are.
- Regenerate `release.yml` with `dist generate` using dist 0.33.0 from the repository's
  `mise.toml`. The expected diff is: the `rust-cache` step gone from `build-local-artifacts`;
  in `host` (tags only), `permissions: attestations: write, contents: write, id-token: write`
  and an `actions/attest` step over `artifacts/*` before `gh release create`. Pull-request runs
  get no `id-token`. Any other diff is a finding to report, not to keep.
- `ci.yml`: a `deny` job running `EmbarkStudios/cargo-deny-action` (pinned by SHA) with
  `check advisories`, on push, pull request, and a weekly schedule. No cache. A root
  `deny.toml` configures advisories only, ignoring `RUSTSEC-2023-0071` with its reason: age's
  `ssh` feature pulls in `rsa`; tagteam refuses RSA identities and `ssh-rsa` recipients
  (`transfer.rs`), so it never decrypts with RSA.
- **Advisory policy** (my call): a vulnerability or unsound advisory fails the job for any
  crate in the graph; an unmaintained notice fails it only for the workspace's direct
  dependencies (cargo-deny's `unmaintained = "workspace"`), since the transitive trees under
  ratatui and age are not tagteam's to replace; a yanked version warns. An advisory inside
  that policy that cannot be fixed on this branch gets a per-ID ignore with its reason in
  `deny.toml`, listed in the MR.
- `ci.yml`'s `check` job keeps its cache (my call): it ships nothing and holds no secret. The
  `deny` job and H7's feature-graph check run in jobs without a cache, so a poisoned cache
  cannot make them pass. If `dist generate` no longer emits `rust-cache`, its pin leaves
  `[dist.github-action-commits]`.
- `SECURITY.md`: how to verify a release with `gh attestation verify`, and the advisory job
  replacing the hand-kept RustSec note (the note's reason moves to `deny.toml`).
- `cc-drift.yml` is not changed: with no release cache, its runner can reach nothing that
  ships.

## H4. One subprocess runner (CC-O-5, PERF-R-2, PERF-R-8)

**Finding.** `security.rs:80-226` (`ProcessRunner`, `Waitable`, `wait_for`, `reap`, `collect`)
and `process.rs:198-335` (`capture`, `wait_for`, `kill_group`, `collect`, `run_discarded`) are
two copies of one runner that have diverged: `security`'s kills only the child, never its
group, and ignores the cancel token. Both poll `try_wait` with a 10 ms sleep, so every child's
exit is noticed 0–10 ms late (about 5 ms per `security` call, 20+ calls per switch). A hung
Keychain read blocks for up to 3 × 5 s + 0.6 s and cannot be interrupted, which holds the
daemon's single scheduler thread and delays SIGTERM.

**Design.**
- `process.rs` keeps the only bounded runner. `run_captured` and `run_discarded` become thin
  wrappers over it, and it gains an optional stdin (bytes written on a thread, as
  `security.rs` does now). `security::ProcessRunner` becomes an adapter from it to
  `RunResult`. The `Runner` trait stays: it is the seam `SecurityCli`'s argv and stdin tests
  use. `Waitable` and `security.rs`'s own `wait_for`/`reap`/`collect` are deleted.
- **Exit without polling.** After spawning, a waiter thread blocks in
  `waitid(P_PID, pid, WEXITED | WNOWAIT)` (retrying on `EINTR`) and sends the result on a
  channel. `WNOWAIT` leaves the child unreaped, so its pid keeps naming its process group until
  the runner reaps it: a `killpg` can never reach a recycled group. libc 0.2.189 exposes
  `waitid`, `P_PID`, `WEXITED` and `WNOWAIT` on Apple and on linux_like (glibc and musl);
  measured on this Mac, the waiter wakes about 3 ms after exit.
- **Waiting.** Without a cancel token in force, the caller waits with one `recv_timeout` to the
  deadline. With one, it waits in 10 ms slices and checks the token between them (today's
  cancel latency); the exit itself is still noticed at once. On exit it reaps with
  `child.wait()`. On the deadline or the token it kills the group with `process.rs`'s
  `kill_group` (`killpg`, then `kill` when `killpg` fails with the leader already a zombie,
  `process.rs:309-325`), then waits for the waiter and reaps. If both kills fail, it returns
  without waiting, as today. A waiter error kills the group, reaps, and reports
  `SpawnFailed`. The drain grace and its rules are unchanged.
- **Which Keychain calls a cancel can interrupt.** §14.1 forbids a cancellation point inside a
  critical span, and some of those spans contain Keychain reads (`Vault::store`'s read and
  read-back, the gate's compare-and-swap read at `refresh.rs:627`, `upsert`'s own verify read
  at `security.rs:478`, the switch's and recovery's reads). A cancelled read there would turn
  into `Unreadable` and divert a successor to `rescue/`, roll a switch back, or (for a verify
  read) push `live.rs:765` into its file fallback. So:
  - `upsert` (its verify read included), `delete` and `delete_service` never take a cancel.
  - A read takes one only inside an opt-in scope: a guard such as
    `cancel::interruptible(&Cancel)` that sets a thread-local token for its lifetime, which
    `SecurityCli::run` reads on the calling thread. Outside a scope, reads behave as today.
  - The scope wraps only usage collection's reads that come before any request, in the
    engine's collect path (`collect.rs`) that the daemon's rounds and `list`/`status` share:
    `stored_token`'s vault read (`collect.rs:866`) and the active account's live-credential
    read. It never covers the refresh gate (`collect.rs:892`) or active-token refresh, which
    are critical spans. Each per-account collect thread sets its own scope. An interrupted read
    is not a usage failure (§14.1, as an interrupted fetch): it records nothing.
  - `SecurityCli` needs no constructor change, so `app.rs` is untouched.
- **Retries.** `live.rs`'s `retrying` (`live.rs:579-590`) does not retry an rc 36 (locked)
  result, which will not change within 600 ms; the daemon's `KeychainBackoff` handles a locked
  keychain between rounds. Inside an interruptible scope it also stops between attempts once
  the token is set.
- `drain`'s `#[cfg(test)]` `DRAINS` counter stays as it is.

## H5. Keychain writes off argv (SEC-C-1, SEC-C-10)

**Finding.** `SecurityCli::upsert` (`security.rs:449-493`) sends
`add-generic-password -U -a … -s … -X "<hex>"` on `security -i`'s stdin, and when that line
exceeds `LINE_LIMIT` (4032 bytes) runs the same command with the hex on argv. Hex doubles the
size, so any credential over about 1,970 bytes of JSON takes argv, where EDR and exec auditing
record it. Claude Code 2.1.303 does exactly the same (same 4032-byte limit; over it, it logs
"Keychain payload … exceeds security -i stdin limit; using argv" and uses argv). tagteam
multiplies the occurrences: its vault items carry the whole live entry, machine-shared MCP
OAuth tokens and plugin secrets included (`switch.rs:2271`, `active.rs:597`,
`lifecycle.rs:767/799`), and every vault store is two upserts. `compose`
(`tagteam-cc/src/shape.rs:194-213`) replaces those keys from live on every activation, so the
vault's copies are never used. Estimated sizes: `claudeAiOauth` alone ~510 B of JSON; with
2.1.303's `designOauth` and `cloudSessionsOauth` ~1,250 B; plus one MCP OAuth server with a
~900-character JWT ~1,700 B (`-X` line ~3,480).

### H5.1 Text instead of hex

- When the credential bytes are valid UTF-8 and contain no NUL, CR or LF, the `-i` line is
  `add-generic-password -U -a "<acct>" -s "<svc>" -w "<text>"`, where `<text>` is the
  credential with `\` written `\\` and `"` written `\"`. Otherwise it is today's `-X "<hex>"`
  line. Item names keep `check_name`'s refusals.
- Verified read-only against `security -i`'s parser: `\"` and `\\` unescape, non-ASCII, `#`,
  `$`, `%` and tab pass through. The reader takes 4095 bytes and runs any remainder as a second
  command, so the 4032-byte limit stays, now on the escaped line. JSON costs about 1.1 bytes
  per byte instead of 2, so the `-i` path holds about 3.8 KB of JSON.
- **Gate.** The first task proves the full round-trip with a `real_keychain` test on a
  throwaway keychain (the `crates/xtask/src/compat/keychain.rs` pattern, restoring the search
  list): a write through `-w` text then `find-generic-password -w` returns the exact bytes for
  JSON containing `"`, `\`, JSON escape sequences such as `\n` and `é`, raw non-ASCII, and
  a ~3.9 KB line; and `-U` with `-w` updates an existing item in place. If the round-trip fails,
  H5.1 is dropped and the branch keeps H5.2 and H5.3; the spec is amended to say so.
- Reads are unchanged: `-w` renders printable text verbatim, and the `-g` disambiguation of
  A.3 still applies to hex-shaped output.

### H5.2 Argv as a logged last resort

- When even the chosen `-i` line exceeds 4032 bytes, `upsert` uses argv, as A.3 and Claude
  Code do, with the same encoding the line would have used (`-w <text>` or `-X <hex>`). It logs
  a WARN naming the service, the account ID (never the email) and the byte length, never the
  value.
- `SECURITY.md` discloses it: when a credential exceeds about 3.8 KB, `security` receives it on
  argv, which macOS shows only to the same user and root but which exec-auditing tools record;
  Claude Code's own writes do the same.

### H5.3 Machine-shared keys stay out of tagteam's copies

- A new provider hook, `vault_form(&self, credential: &[u8]) -> Cow<[u8]>` (name is the
  builder's), returns the credential as tagteam keeps it. The default returns the bytes
  unchanged. Claude Code's drops the five `MACHINE_SHARED_KEYS`, keeping every other key and
  its order (`preserve_order`). A credential that is not a JSON object (a managed API key) is
  returned unchanged.
- Every write of a credential into tagteam's own storage applies it: `Vault::store` (all
  callers) and `rescue/` envelopes (H6 included). `displaced/` files stay verbatim: they keep
  exactly what was not tagteam's to keep, and they are plain files that never reach
  `security`. `Vault::store`'s
  read-back compares the form it wrote. The builder chooses how `store` reaches the provider;
  if the choice changes `store`'s signature, `switch.rs:2271` changes only in its arguments
  (H1.2).
- Comparisons of live or received bytes against the vault:
  - `persist_received`'s "the vault write may have landed" check (`refresh.rs:578`) compares
    `vault_form(received.bytes)`.
  - `OutgoingFacts.bytes_equal_vault` (`switch.rs:2251`) and the same comparison at
    `recover.rs:390` stay as they are. They become false when the live entry carries
    machine-shared keys, but `decide_outgoing` (`classify.rs:55`) ORs `fp_equal_vault`, which
    stays true for every token-bearing credential, so the outcome is unchanged. A tokenless
    live credential would classify as `Wiped` instead of `Ours`, with the same action
    (`Nothing`). A test pins both cases.
- Existing vault items keep their old bytes until their next store; nothing migrates them.
  `settle_rescues` adopting an old rescue stores the vault form.
- **Exports** (my call): `shape::export_credential(secret, full = true)`
  (`tagteam-cc/src/shape.rs:279-296`) and the fake provider's copy also drop the five keys.
  Without this, a `--full` export of the live account (`export.rs:431-469` picks the live
  credential through `transfer::newest_live`, `transfer.rs:793-810`, whenever Claude Code
  rotated the token since the last capture) or of a session-owned account would still carry
  MCP OAuth tokens and plugin secrets. Their import would drop them anyway, since the
  imported credential is stored in vault form. `--full` therefore keeps every
  account-scoped key (`trustedDeviceToken` included) and no machine-shared one, whatever the
  source. Default exports were already slimmed to `{claudeAiOauth}`.

## H6. Refresh write-ahead (SEC-C-4)

**Finding.** After the token endpoint returns a successor, its only copy is in memory until
`Vault::store` verifies it: on macOS about 8 `security` spawns (`refresh.rs:627`, `:343`,
`vault.rs:157`, two upserts each with its verify read, the read-back at `vault.rs:172`). A
SIGKILL in that window (launchd's exit timeout at logout, `kill -9`, the OOM killer, a dead
battery) loses the only valid token, and the account needs a re-login. `Received`'s `Drop`
covers panics, not kills. Today `rescue/` is written only after the vault fails
(`refresh.rs:573-591`).

**Design.**
- As soon as a received successor is known to belong to the account, before anything else
  (the gate's vault re-read in `persist_successor` included), write its rescue envelope
  (§6.3's format, the vault form of H5.3) with the existing fsynced 0600 writer. This covers
  the gate and active-token refresh's `persist_received`. After the vault write verifies,
  delete it. If the write-ahead fails, log it and go on as today; it is best-effort.
- A foreign successor is never written ahead. It still goes only to `displaced/` (§7.4).
- If the vault write fails, the written-ahead envelope is the rescue: `received.keep()` must
  not write a second copy.
- **Landed rule.** Before its adoption check, `settle_rescues` deletes a rescue whose own
  (successor) fingerprint equals the vault's current fingerprint. Claude Code's fingerprint
  hashes the refresh token (`shape.rs:50-59`), so the vault then already holds a valid
  generation with the same refresh token, and deleting the rescue loses at most an access
  token that refresh token re-obtains. The rule covers two cases:
  - a crash after the vault write landed and before the delete (the vault holds the rescue's
    bytes; `active.rs:542-548` already handles this for the live account only);
  - a same-generation successor (a reply without a new refresh token keeps the fingerprint,
    so today's check `current == predecessorFp` would re-adopt it, possibly over a newer
    access token written since).
- **Every place that adopts or picks a rescue applies the landed rule first.** Adopters delete
  such a rescue; pickers, which only read, skip it.
  - Adopters: `settle_rescues`, and the live account's `reconcile_active`
    (`active.rs:526-557`, §7.5 step 3, which today adopts a rescue whose `predecessorFp`
    equals the live fingerprint before its own landed check).
  - Pickers: the shared `transfer::successor` (`transfer.rs:784`) and
    `transfer::newest_live` (`transfer.rs:794`), so that export's live path
    (`export.rs:463`) and its session-owned and inactive paths (`export.rs:494-550`, through
    `successor`) all skip it. The rule lives in those two functions, not in their callers.
  - For the live account the comparison is against the vault's current fingerprint, which the
    live entry matches whenever the rule can apply.
- Routine write-ahead files log at DEBUG; `rescue.rs:115`'s INFO line stays for rescues kept
  because the vault failed.
- `doctor`'s `pending.rescue` check is unchanged; a landed rescue is gone before it looks.

## H7. Test doubles out of the release library (CC-O-4, ARCH-11)

**Finding.** `tagteam-provider` compiles and re-exports `FakeKeychain` (`keychain.rs:207`, with
injected panics), `FakeClock` (`clock.rs:21`), `ScriptedHttp` and `RecordedRequest`
(`http.rs:211`, `:181`), `FakeProcess` and `FakeProcessProbe` (`liveness.rs:449`, `:461`) and
`ScriptedSpawner` (`process.rs:511`) unconditionally (`lib.rs:44-62`), so the release binary
links them. `FileKeychain`, `MockServer` and `lock-log` are already feature-gated.

**Design.**
- A `test-doubles` feature on `tagteam-provider`. The types above, their impls and their
  re-exports are `#[cfg(any(test, feature = "test-doubles"))]`.
- It is enabled only from dev-dependencies: added to the existing dev-dependency feature lists
  of `tagteam`, `tagteam-cc`, `tagteam-engine` and `tagteam-daemon`, and as dev-dependency
  lines in `tagteam-tui`, `tagteam-fake` and `xtask` (where xtask's uses are under
  `cfg(test)`).
- `Env::for_test` and `NoHttp` stay ungated: `NoHttp` is production code (the statusline), and
  `xtask/src/compat/guard.rs:77` uses `Env::for_test` outside tests. dist builds with
  `--workspace`, so a feature any member enables in normal dependencies ships.
- CI asserts that `test-doubles` is not enabled in the normal-dependency feature graph of the
  workspace (for example `cargo tree --workspace -e normal,features -i tagteam-provider`).

## H8. Daemon idle wakeups (PERF-R-3)

**Finding.** An idle daemon wakes about 21 times a second, and 10 more per open connection
(a subscribed TUI keeps one): the accept thread polls with a 100 ms timeout
(`server.rs:50, 230-245`); the notifier waits in `recv_timeout(100 ms)` only to check its stop
flag (`notify.rs:50, 183-191`), and on macOS it always runs (`notify.rs:66-72`,
`daemon.rs:147-150`); each connection reads with a 100 ms timeout (`server.rs:525`, loop at
`537-577`); the scheduler slices at 1 s (`scheduler.rs:18, 185-187`).

**Design.**
- **Notifier.** Blocks in `recv()`. `Notifier::drop` sends an explicit stop message; dropping
  the sender is not enough, because `Started.notifier` (`daemon.rs:247`) drops the `Notifier`
  before its queue when `Started` is dropped without `serve`.
- **Accept thread.** `poll`s the listener and an internal wake pipe, with no timeout.
  `Hub::close_socket` and `Server::close` write the pipe. A signal still reaches it the way it
  does now: the scheduler notices the flag, emits `stopping`, and closes the socket.
- **Connections.** Each `poll`s its socket, a per-subscriber wake descriptor that `Hub::queue`
  writes after a successful `try_send`, and a server-stop descriptor that `Server::close`
  writes. Events reach a subscriber at once instead of within 100 ms. The 30 s pre-subscribe
  idle timeout and the 2 s write timeout stay, as `poll` timeouts. Decision 9's order holds:
  open connections stay served until shutdown, so each subscriber reads `stopping`, and the
  full-queue drop is unchanged.
- Wake descriptors are non-blocking, close-on-exec pipes; a write that finds the pipe full is
  ignored (the reader is already due to wake). At most 16 connections, so at most 32 extra
  descriptors.
- **Unchanged:** the flag-only signal handler (M3a plan Decision 2), the scheduler's 1 s slice
  (§11.4's suspended clock; D4.2's "a setting takes effect within about a second"), the 2 s
  store watch.
- Idle total: about 1.1 wakeups a second, whatever number of clients is subscribed.

## H9. `tagteam run` wakeups (PERF-R-4)

**Finding.** `run`'s wait loop is `try_wait` + `sleep(50 ms)` + `forward(cancel.take())`
(`run.rs:23, 462-474`), and the token report's launch poller runs `observe_launch` (an
`fs::metadata` of the session record, `evidence.rs:432`) and sleeps 50 ms (`run.rs:287-345`):
40 wakeups and 20 `stat`s a second for every supervised session, for its whole life. No reason
is recorded for 50 ms itself; it came from M4b plan Decision 1's tick and was reused for the
record check (commit 600fd9e). `run::wait` is also the TUI's wait for `tagteam run`
(`tui_host.rs:106`).

**Design.**
- A waiter thread, the same `waitid(WNOWAIT)` primitive as H4, reports the child's exit at
  once. The main thread stays the only one that reaps, so `forward`'s rule that the pid
  "names the child or its zombie" (`run.rs:445-447`) still holds. A blocking `child.wait()` on
  another thread is not allowed: it would reap the child and let a forwarded signal reach a
  recycled pid.
- The main thread waits on the waiter's channel with a 250 ms timeout. On each wake it
  forwards `cancel.take()` as today; on exit it reaps. Exit is noticed immediately (today
  within 50 ms); a signal is forwarded within 250 ms (today 50 ms).
- The launch poller ticks every 250 ms, on its own thread as now. The 2 s sighting write
  (`run.rs:27`) is unchanged. The poller must not move onto the wait thread: its store writes
  can block on SQLite's write lock, and `wait_polling` (`run.rs:259-268`) keeps them off that
  thread so a busy store never delays the wait, the signals it forwards or the exit stamp.
- **Attribution.** A tenure ends at its last confirmation (token §6.4), so at most the
  responses in the last 250 ms before a tenure ends go unsighted. With `run`'s launch open,
  those have two candidates and are left unattributed, never given to the wrong account
  (token §7.2, success criterion 2). In interactive use a human pause makes this moot; it
  matters only for a headless `claude -p` whose last answer lands within 250 ms of exit.
- Result: about 8 wakeups a second, down from 40.

## H10. Token report (PERF-D-1, PERF-D-3)

### H10.1 Compact rows (PERF-D-1)

**Finding.** `token_usage` (`store/tokens.rs:880-909`) collects every response of the range
into a `Vec<UsageRow>` with four owned strings per row (`session_id`, `project`, `model`,
`speed`; `:334-353`), then `group` builds a second `Vec<Resolved>` (`view/group.rs:84-106`).
The default `month` range is 12 calendar months (`view/mod.rs:341-356`, token §11.1): about
1.5M rows at the spec's own rate, estimated ~0.5 GB and 2–3 s, against a budget that covers
only 30 days (`tests/tokens_bench.rs:29-31`, 200 ms). The TUI repeats the query every 60 s and
after each index change.

**Design.**
- Rows carry no owned strings: session and model as their integer ids in `token_sessions` and
  `token_models`, resolved once per report; `project` interned per session; `speed` as an
  enum or interned id. The Vec and the read order stay.
- Not streaming: an open usage statement would hold the store's mutex, which `Evidence::load`
  and `series_blocks` retake (a std `Mutex` is not re-entrant), and it would fix the
  connection's snapshot before the pending files are read, breaking the order
  `view/mod.rs:364-371, 402-405` rely on (token §7.2). Block reports need every response time
  anyway.
- **Budget.** `tokens_bench.rs` gains a 12-month case at the spec's rate (~1.5M responses).
  The first task measures today's time and peak memory; the target is ≤ 1 s release-mode for
  the report query and grouping. If compact rows miss it, the builder stops and reports before
  trying anything larger. The budget the measurement supports is written into token §1.1/§15.

### H10.2 Samples queries on the key (PERF-D-3)

**Finding.** `SAMPLES_OF_ALL` (`store/usage.rs:209-210`,
`WHERE account_id = ?1 AND fetched_at >= ?2 ORDER BY fetched_at, window`) cannot use the key
`(account_id, window, fetched_at)` (`schema.sql:52`) as a range: it reads every sample of the
account (up to ~345k at the 180-day retention) and sorts them. The block report's
`short_resets` (`group.rs:551`) then keeps only the short window's rows.

**Design.** Code only, no index:
- One store query lists an account's stored window keys with a skip-scan over the key (a
  recursive CTE stepping `window > ?` with `LIMIT 1`). It finds every key with samples,
  including windows the latest reading no longer has. `Provider::describe_window` only
  describes a key it is given; it cannot list them.
- `short_resets` (`group.rs:543-560`) lists the keys, keeps those `describe_window` calls
  `WindowKind::Short` (its existing per-key cache stays), and reads each with
  `SAMPLES_OF_WINDOW`, a key range with no sort.
- `history` (`views.rs:738`) lists the keys and reads each with `SAMPLES_OF_WINDOW`, merging in
  `(fetched_at, window)` order.
- EXPLAIN QUERY PLAN tests pin both, next to the existing `SAMPLES_OF_WINDOW` pin
  (`usage.rs:811-828`), with SQLite as bundled.

## H11. Small fixes

### H11.1 `add-token -` echoes the token (SEC-C-7)

`app.rs:1965-1971` reads `-` on a terminal through `prompt::read_terminal_line`
(`prompt.rs:260-271`), which echoes. `read_terminal_line` reads with echo off
(`read_secret`'s mechanism, `prompt.rs:392`) and writes the newline itself. Its other caller,
`tui_host.rs:242` (`wait_for_enter`), only waits for Enter, so not echoing stray keys is fine.
`tests/signals.rs:419` (Ctrl-C on a tty) must still pass.

### H11.2 `add-token <TOKEN>` on argv (SEC-C-2, SEC-S-2)

The literal form stays (§10.2). When the token came from argv, `add-token` prints one stderr
line: the token was visible to other local users and in shell history, and `-` or the prompt
avoids that. This is the one line in `app.rs` (`:1980`) this branch changes. `add-token`'s help
text and `SECURITY.md` recommend `-` or the prompt.

### H11.3 `primaryApiKey` in a readable `~/.claude.json` (SEC-C-3)

On Linux every API-key activation, and on macOS a Keychain refusal, splices `primaryApiKey`
into `~/.claude.json` keeping that file's mode (`live.rs:939-951`, `config.rs:115`). That file
is commonly 0644 (it is on the author's Mac), and Claude Code keeps the key there itself, so
refusing would break API-key accounts on Linux, and chmod would change Claude Code's file (§3,
§9.5). Before such a splice, `live.rs` checks the resolved target's mode and, when group or
other bits are set, logs a WARN with the `chmod 600` fix. `SECURITY.md` documents the
exposure and the fix. A `doctor` check would live in `doctor.rs` and is left out (H1.2).

### H11.4 Control characters in human output (SEC-S-3)

One helper in `tagteam-core` (so `tagteam-cc` can use it too) replaces C0 (except tab), DEL and C1 characters with
U+FFFD. It is applied where human output interpolates text tagteam did not write: the session
report's PROJECT column (`tokens_cmd.rs:621`, from a transcript's `cwd`), unmanaged login
emails (`statusline.rs:59`, `render.rs:431`). The token endpoint's `error_description` is
sanitized where it is captured (`tagteam-cc/src/oauth.rs:122-128`), so its renderers need no
change. `--json`, CSV and the TUI (ratatui already drops control graphemes) are unchanged.

### H11.5 The socket client trusts any server (SEC-S-8)

`client.rs:60-69` (`connect_at`) checks the server's uid with the existing `peer_uid`
(`server.rs:336-373`) against `geteuid()`. A mismatch is `Connect::NoDaemon`, logged at WARN.

### H11.6 SECURITY.md: superseded tokens (SEC-C-12)

`SECURITY.md:58` says tagteam never sends a superseded token. It becomes "never sends a
generation it knows was superseded", with one sentence on §7.3's deliberate retry after an
ambiguous failure.

### H11.7 An INFO line every minute (PERF-R-12)

`tokens/refresh.rs:573-582` logs "refreshed the token index" at INFO only when the pass read a
file, added, updated or removed a response, changed evidence, or stopped incomplete; otherwise
at DEBUG.

## H12. Findings not acted on

| Finding | Reason |
|---|---|
| SEC-C-5 root guard trusts an overlay root | §5 defines the overlay exception on purpose; an owner check would refuse root in a container with a bind-mounted home, the case it exists for. A footgun, not an escalation. |
| SEC-C-6 import keeps unknown credential keys | A.4 treats unknown keys as account-scoped by design and §13.3's `--full` round-trips them; the impact on Claude Code is unverified. |
| SEC-C-8 no export passphrase floor | The user chooses their own passphrase; the check would live in `app.rs`. |
| SEC-C-9 existing directories not tightened to 0700 | `doctor` already warns with a fix (§13.6); `ensure_private_dir` also runs on Claude Code's own directory, which tagteam must not chmod (§3). |
| SEC-C-11 displaced and rescue entries kept forever | `doctor` already lists every one with a fix line; an age threshold would live in `doctor.rs`. |
| SEC-S-4 unverified installers, tap token on disk | dist's stock generated workflow; hand edits are undone by `dist generate`. Michael checks that `HOMEBREW_TAP_TOKEN` is fine-grained and limited to the tap. |
| SEC-S-6 `test-support` env overrides | Release builds compile the stubs; the fix would be in `app.rs`; setting the environment already means running as the user. |
| SEC-S-7 argv over ~2 KB | The same as SEC-C-1 (H5). |
| SEC-S-9 `contents: write` for PR runs | dist's generated default; fork PRs get a read-only token and the maintainer owns same-repo branches. |
| SEC-S-10 `cargo publish` verifies with the token present | A compromised build dependency could backdoor the binary anyway; verification catches packaging errors. |
| PERF-R-5 statusline cache misses commit and fire `changed` | The cache is a recorded decision (§13.5); the miss rate is unmeasured. Revisit on measurement. |
| PERF-R-6 quarantine checks every tick | §11.2 step 1 releases quarantines on every tick; Q spawns a minute, only in `auto` mode. |
| PERF-R-7 sessions scanned twice | Microseconds to a few ms every 5 s; revisit on measurement. |
| PERF-R-9 TUI input slice 50 ms | Deferred to the TUI UI/UX review now under way, which will likely redesign `crates/tagteam-tui`. Note for it: the pause handshake test (`input.rs:418`) bounds the slice. |
| PERF-R-10 TUI view re-reads, per-frame layout | Deferred to the TUI UI/UX review. |
| PERF-R-11 two DNS lookups per request | The pre-lookup is how a DNS failure is known to be `PreSend` (§4); one request a minute. |
| PERF-R-13 no perf budget for switch, daemon, run | Switch spawn counts belong with PERF-R-1 (refactor); wakeup-counting tests are flaky. H8 and H9 get behaviour tests instead. |
| PERF-D-4 full index state every 5 s | Measured ~6 ms at 10× today's size; about 1 ms at real size. |
| PERF-D-5 daily samples prune scans the table | Measured 40 ms at the 180-day cap (1.7M rows), once a day; an index needs schema v4. |
| PERF-D-6 no-op refresh allocations | 1–2 ms at 2.5k files, once a minute. |
| PERF-D-7 launches loaded whole | 0.8 ms measured with 3,000 launches. |
| PERF-D-8 live identity re-read per view | The cache cannot hold a full `Identity`, and the switch-path readers must read the file (`switch.rs`). |
| PERF-D-9 scalar newline search, chunk zero-fill | Within the cold-index budget; the fix adds a direct dependency. |
| PERF-D-10 no `ANALYZE`/`PRAGMA optimize` | Measured 1.4× in a synthetic worst case; `PRAGMA optimize` writes, which fires `changed`. |
| PERF-D-11 pace recomputed per view | A few ms per account at most. |
| PERF-D-12 no `prepare_cached`, small tables | Tens of microseconds per statement; tables that grow only with switches. |
| PERF-D-2 heartbeat forces a re-query | Store and daemon side kept as intended (H2 decision 3). The TUI side (`screens/tokens/index.rs` re-querying on every `changed_at`) is deferred to the TUI UI/UX review. |

## H13. Amendments to recorded decisions

Made in small hunks in the same branch, as D16 did.

- **Core spec**
  - **A.3 Write:** the `-i` line carries `-w "<escaped text>"` for UTF-8 credentials without
    NUL, CR or LF, else `-X "<hex>"`; argv only when the line still exceeds 4032 bytes, with a
    WARN, as Claude Code does (H5.1, H5.2).
  - **A.3 Active reads:** no retry after rc 36 (H4).
  - **A.4:** machine-shared keys never enter the vault or `rescue/`; `displaced/` stays
    verbatim (H5.3).
  - **§6.2, §6.3:** the vault holds the vault form; `rescue/` is also written ahead of every
    vault write of an own successor; the landed and same-generation rules (H6).
  - **§7.3 step 6, §7.5 step 5:** write-ahead order (H6).
  - **§7.5 step 3's table:** the landed rule comes before adoption (H6).
  - **§12.5:** `run` notices exit at once; it forwards signals and checks the session record
    every 250 ms (was 50 ms) (H9).
  - **§13.3:** `--full` keeps every account-scoped key and no machine-shared one, whatever the
    credential's source (H5.3).
  - **§14.1:** a Keychain read inside usage collection is a cancellation point; Keychain writes
    never are (H4).
- **Daemon spec**
  - **D4.6:** "The accept and connection threads poll with 1 s timeouts" becomes: they block on
    their descriptors and wake pipes (H8). This also supersedes daemon-b plan Decision 2's
    100 ms read timeout.
  - **D6:** the notifier thread blocks until a notice or its stop message (H8).
- **Token report spec**
  - **§6.1, §7.4 A3:** the record check runs every 250 ms (was 50 ms); the unattributed tail is
    at most 250 ms (H9). Supersedes M4b plan Decision 1's 50 ms tick for `run`.
  - **§1.1, §15:** the 12-month report budget H10.1 measures.
- **SECURITY.md:** H3, H5.2, H6 (on macOS a refreshed token sits in a 0600 file for about
  100 ms per refresh), H11.2, H11.3, H11.6.

## H14. Testing

- **H3:** the regenerated workflow's diff is exactly H3's; the `deny` job passes on the branch;
  the feature-graph check of H7 runs in CI.
- **H4:** the existing real-process tests (`process.rs:790/810/876`, `security.rs`'s grandchild
  and process-group tests) pass on the unified runner, on macOS and in Linux CI. New: a
  zombie-leader group is killed via the `kill` fallback; a cancel interrupts a read inside an
  interruptible scope and never a read outside one; a cancel set before `upsert` neither stops
  the write nor its verify; rc 36 is not retried.
- **H5:** H5.1's `real_keychain` round-trip gate; scripted `Runner` tests for the line chosen
  (text, hex for bytes with NUL/CR/LF, argv with its WARN over the limit); `vault_form` drops
  exactly the five keys and keeps order; `Vault::store` writes and verifies the vault form;
  `persist_received`'s landed check with a received credential carrying machine-shared keys;
  `--full` export of a live-sourced and a profile-sourced credential carries no machine-shared
  key;
  the classify outcome with a live entry that carries them (H5.3).
- **H6:** with the engine's test hooks: a crash after the write-ahead and before the vault
  write is adopted by the next gate pass; a crash after the vault landed leaves a rescue the
  landed rule deletes; a same-generation rescue is deleted, never adopted over the vault; a foreign
  successor writes nothing ahead; a failed write-ahead still stores to the vault. For the live
  account: a crash-left same-generation rescue is deleted by `reconcile_active`, never
  published over the live entry, and skipped by `transfer::newest_live` and
  `transfer::successor` (a live, a session-owned and an inactive export each pick the vault).
- **H7:** the workspace's tests pass with the feature enabled only from dev-dependencies; the
  CI feature-graph check fails if a normal dependency enables it.
- **H8:** the notifier's thread ends when the `Notifier` is dropped, including when `Started`
  is dropped without `serve`; `close` wakes the accept thread; a queued event reaches a
  subscriber without waiting out a read timeout; shutdown still delivers `stopping` to every
  subscriber (Decision 9) and drops a full queue's subscriber.
- **H9:** the existing `run` and TUI-wait tests pass; a signal is forwarded to a live child and
  never after it was reaped; the child's exit is reported without waiting a tick.
- **H10:** the 12-month bench (release only, as today); every existing report test's output is
  unchanged; the EXPLAIN pins.
- **H11:** one test per fix: no echo on `add-token -` (pty), the argv warning, the WARN on a
  0644 `~/.claude.json`, the sanitizer on each site, the client's uid check.

## H15. Risks and open items

- **H5.1 depends on an unverified round-trip.** Its gate is the first task; failure drops H5.1
  only.
- **Linux `waitid`** is verified in libc's declarations, not yet run; Linux CI covers it.
- **Sizes in H5** are estimates; no real Keychain entry was read.
- **H6 narrows the kill window; it does not close it.** The response-to-envelope write (one
  small fsynced file) remains, and a failed write-ahead falls back to today's exposure for that
  refresh.
- **H6 adds plaintext on macOS:** a refreshed token sits in a 0600 file for about 100 ms per
  refresh (FileVault-encrypted at rest; a local snapshot taken inside that window keeps a
  token that the next refresh supersedes). Decision 4 accepted it.
- **Merge order:** this branch merges first; `review-refactor` rebases onto it. The two
  out-of-ownership touches (H1.2) are listed in the MR.

## Errata

Found while planning and building; each is listed in the MR.

- **H1.2:** the branch touches `switch.rs` a third time: one line at the direct branch of
  step 2 (`switch.rs:2043`) compares the live entry and the target's vault bytes both in
  vault form, or `switch --force` onto the live account would displace its own entry (H5.3).
- **H5.1:** the hex line is never shorter than the text line, so `-X` is used only for bytes
  that cannot be text; past the limit, argv uses the same flag as the line would have: `-w`
  with the credential's own text, unescaped (argv needs no quoting, so the line's escaping
  does not apply), or `-X` with the hex.
- **H6:** `export.rs:494-550` is the session-owned export path; an idle account's export goes
  through `settle_rescues`. All three paths apply the landed rule.
- **H6:** the WARN for a failed write-ahead is logged only after the vault write settles,
  never while the successor exists only in memory (§7.3 step 6); and `reconcile_active`
  deletes landed rescues best effort, so a rescue it cannot delete never stops the live
  repair.
- **H8:** the wake pipes add up to 36 descriptors, not 32: two shared pipes beside the 16
  connections' own.
- **H10.1:** compact rows alone missed the 1 s budget; the range read also names its index
  (`INDEXED BY token_usage_by_at`), pinned by an EXPLAIN test. Measured on a 12-month report
  of about 1.52M responses: 1.41 s median and 451 MiB peak before; 0.75 s and 194 MiB after
  (release build, Apple silicon, loaded machine); the 30-day case stayed within its 200 ms
  budget.
- **H11.4:** `render.rs:456-458` (the run shell's unmanaged session login) is sanitized
  beside `render.rs:431`.
