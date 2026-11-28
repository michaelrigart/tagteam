# Toolchain and dependency update, November 2026: implementation plan

**Status:** Approved

**Goal:** Rust 1.100.1, cargo-dist 0.34.0, rusqlite 0.43, jiff 1.0.3, cargo-deny-action v2.1.5
and every package in the lockfile at its newest release.

**Spec:** `docs/superpowers/specs/2026-11-28-tagteam-toolchain-deps-design.md`

**Size:** about 45 lines of Rust, added plus deleted (the jiff rename in nine files); about 30
lines of manifest and workflow configuration; about 370 lines of generated `Cargo.lock` churn,
as the spec's trial on `81507e2a` measured.

## Global Constraints

- The only Rust change is jiff's rename, `Zoned` to `ZonedDateTime` and `to_zoned` to
  `to_zoned_datetime`, and what rustfmt reflows around it. A task that needs any other source
  change stops and reports: that is a spec change.
- Every pinned version is the one the spec's table names. `Cargo.lock` gains exactly two
  packages, `ar_archive_writer` and `object`.
- unicode-normalization, ratatui, crossterm, generic-array and kem do not move.
- Run cargo with Rust 1.100.1 itself: mise exports `RUSTUP_TOOLCHAIN=1.99.1`, which beats
  `rust-toolchain.toml`. Put `~/.rustup/toolchains/1.100.1-aarch64-apple-darwin/bin` first on
  `PATH` and unset `RUSTUP_TOOLCHAIN`; confirm with `cargo --version`. This cargo takes no
  `+<version>`: for the Rust 1.89 checks, do the same in a subshell with
  `~/.rustup/toolchains/1.89.0-aarch64-apple-darwin/bin`, and confirm both `cargo --version` and
  `rustc --version` say 1.89.0 there.
- Tasks 1 to 6 are one commit each, and the workspace builds with `--locked` after each. Tasks
  1, 2, 3, 4 and 6 run in that order: they share `Cargo.toml`, `Cargo.lock` or
  `CONTRIBUTING.md`. Task 7 commits the records.
- Editing `mise.toml` (Task 6) makes mise's shims refuse to run in this checkout until Michael
  trusts the file again. Nothing after it may go through a shim: cargo comes from the toolchain
  directory above, and dist and cargo-deny are called by their full paths under
  `~/.local/share/mise/installs/`.

## Tasks

| Task | Depends on |
|---|---|
| 1 | none |
| 2 | 1 |
| 3 | 2 |
| 4 | 3 |
| 5 | none |
| 6 | 4 |
| 7 | 5, 6 |

### Task 1: Pin Rust 1.100.1

**Depends on:** none

- [ ] Check first: with 1.100.1, `cargo check -p tagteam --locked` prints the
  `package.homepage is redundant` warning.
- [ ] `rust-toolchain.toml`: `channel = "1.100.1"`.
- [ ] Remove `homepage.workspace = true` from `crates/tagteam/Cargo.toml` and `homepage` from
  `[workspace.package]` in `Cargo.toml`.
- [ ] `README.md` and `CONTRIBUTING.md`: Rust 1.99.1 becomes 1.100.1, including the
  `rust:1.99.1` image and `RUSTUP_TOOLCHAIN=1.99.1` in the Docker command.
- [ ] The check no longer prints the warning; `cargo fmt --all --check` and clippy's three CI
  configurations pass with `-D warnings`.

### Task 2: rusqlite 0.43

**Depends on:** 1

- [ ] `Cargo.toml`: `rusqlite = { version = "0.43", … }`; the minimum-supported-Rust comment and
  `CONTRIBUTING.md` name rusqlite 0.43.
- [ ] `cargo update rusqlite@0.41.1 libsqlite3-sys@0.39.1`: rusqlite 0.43.0, libsqlite3-sys
  0.40.0, and whatever those two require; nothing else moves.
- [ ] `cargo test -p tagteam-engine --locked` passes (the store tests).

### Task 3: jiff 1.0.3

**Depends on:** 2

- [ ] Check first: with `jiff = { version = "=1.0.3", … }` in `Cargo.toml` and `cargo update
  jiff@0.2.38`, `cargo check --workspace --all-targets --locked` fails on `jiff::Zoned` and
  `to_zoned` in `crates/tagteam-core/src/tokens/bucket.rs`.
- [ ] Rename `Zoned` to `ZonedDateTime` and `to_zoned` to `to_zoned_datetime` wherever a Rust
  file under `crates/` names them as jiff's, then `cargo fmt --all`.
- [ ] Clippy's three CI configurations pass with `-D warnings`.
- [ ] `cargo test -p tagteam-core -p tagteam-cc -p tagteam-engine -p tagteam-tui --locked`
  passes (the token report's days, months and time zones, and the TUI's dates).

### Task 4: Refresh the lockfile

**Depends on:** 3

- [ ] `cargo update`.
- [ ] Freshness, in this same session (the spec's §6): `cargo update --dry-run` reports nothing
  to update, and every crates.io dependency a workspace manifest names is locked at the newest
  stable release crates.io lists, unicode-normalization excepted.
- [ ] Against `81507e2a`, `Cargo.lock` gains `ar_archive_writer` 0.5.5 and `object` 0.39.1 and
  no other package name, and loses `atomic`, `bytemuck` and the three `defmt` crates.
- [ ] `cargo deny check advisories` passes with cargo-deny 0.22.2.

### Task 5: cargo-deny-action v2.1.5

**Depends on:** none

- [ ] Confirm `git ls-remote --tags https://github.com/EmbarkStudios/cargo-deny-action
  'v2.1.5*'` peels to `bb119f768ea239e765aeaef5769558d4f21213aa`.
- [ ] `ci.yml`: `EmbarkStudios/cargo-deny-action@bb119f768ea239e765aeaef5769558d4f21213aa # v2.1.5`.

### Task 6: cargo-dist 0.34.0

**Depends on:** 4

- [ ] `mise.toml` and `dist-workspace.toml`: 0.34.0; `CONTRIBUTING.md` names it.
- [ ] Regenerate with the pinned binary,
  `~/.local/share/mise/installs/cargo-dist/0.34.0/*/dist generate`.
- [ ] The diff of `release.yml` is exactly the installer URL and the two `uses: $/…` lines.
- [ ] `dist generate --check` passes; `dist build --artifacts=global --tag v20261111.1935.0`
  writes a formula whose `homepage` is the repository URL.

### Task 7: Pull request, records and verification

**Depends on:** 5, 6

The local checks are every line of the spec's §6 that runs locally, with totals: the
package-name, action and Rust-diff checks included, and the four Rust 1.89 checks of CI's `msrv`
job in the 1.89.0 subshell. Freshness is not among them: Task 4's result stands, as §6 says.

- [ ] Run the local checks on the last commit of Tasks 1 to 6, then push the branch and open the
  Draft PR.
- [ ] One commit sets `**Status:** Implemented` with the PR's URL in the spec and in this plan,
  and ticks this plan's boxes. Push it.
- [ ] On that commit: the local checks again, and their totals are the ones reported; the PR's
  head is the same commit; its CI is green, the release workflow's `release-smoke` job included.
- [ ] If the release workflow fails on a `$/` call, whether `release-smoke` fails to run or
  GitHub rejects the workflow before any job starts: revert Task 6's commit, record cargo-dist
  as held back in the spec, push, and repeat the previous step on the new head with dist 0.33.1.
