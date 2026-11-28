# tagteam — toolchain and dependency update, November 2026

**Status:** Approved
**Date:** 2026-11-28
**Scope:** `rust-toolchain.toml`, `Cargo.toml`, `Cargo.lock`, `crates/tagteam/Cargo.toml`,
`mise.toml`, `dist-workspace.toml`, `.github/workflows/release.yml` (generated),
`.github/workflows/ci.yml`, the version numbers in `README.md` and `CONTRIBUTING.md`, and the
nine Rust files that name jiff's `Zoned` type or its `to_zoned` method.
**Size:** about 45 lines of Rust, added plus deleted, all of it one rename; about 30 lines of
manifest and workflow configuration. The generated `Cargo.lock` is counted apart: about 370
lines of churn. The estimate rests on a trial of the whole update on `81507e2a`.

Versions and dates are as read at each registry on 2026-11-28.

---

## 1. Purpose

Bring the Rust toolchain, the release tool, every dependency, the lockfile and one CI action to
their newest release. tagteam's behaviour does not change. One piece of package metadata does:
the `tagteam` crate no longer declares a `homepage` (§4.1).

It is not a small change: it regenerates the release workflow, which publishes the archives, the
Homebrew formula and the crates; it takes a major version of jiff; and it adds two crates to the
build.

## 2. Newest, not seven days old

The standing rule moves a pinned version only to a release at least seven days old. Michael set
it aside for this update on 2026-11-28: everything goes to its newest release. Several of the
versions below are younger than seven days; §3 and §4.6 mark them.

## 3. What moves

| What | From | To | Released | Note |
|---|---|---|---|---|
| Rust (`rust-toolchain.toml`) | 1.99.1 | 1.100.1 | 2026-11-27, young | §4.1 |
| cargo-dist (`mise.toml`, `dist-workspace.toml`) | 0.33.1 | 0.34.0 | 2026-11-02 | §4.2 |
| rusqlite | 0.41 (locked 0.41.1) | 0.43 (locked 0.43.0) | 2026-11-15 | §4.3 |
| jiff | =0.2.38 | =1.0.3 | 2026-11-24, young | §4.4 |
| EmbarkStudios/cargo-deny-action (`ci.yml`) | v2.1.1 | v2.1.5 | 2026-11-27, young | §4.5 |
| `Cargo.lock` | | every package at its newest | | §4.6 |

The minimum supported Rust stays 1.89: the lockfile is resolved for it, jiff 1.0 declares 1.85,
and CI's `msrv` job checks it.

## 4. Decisions

### 4.1 Rust 1.100.1

1.100.1 fixes a type-inference regression in 1.100.0. clippy 1.100 adds no finding.

Cargo 1.100 adds its own manifest lints. One fires: `homepage` in `crates/tagteam/Cargo.toml`
is redundant with `repository`, because both hold the same URL. The fix removes
`homepage.workspace = true` from that crate and `homepage` from `[workspace.package]`. dist
falls back to `repository`, so the Homebrew formula's `homepage` line is unchanged (checked by
generating the formula both ways). `cargo metadata` then reports `homepage: null` for `tagteam`,
and the next publication to crates.io carries no homepage; its repository link is the same URL.
dist's plan step then warns that the Homebrew job has no homepage, in the release log only,
while still writing the formula's from `repository`: one warning in release logs in place of
one on every build. The alternative, allowing the lint in a `[lints.cargo]` table, is rejected.

### 4.2 cargo-dist 0.34.0

Regenerating `release.yml` with 0.34.0 changes three lines:

- the installer URL, to v0.34.0;
- the calls to the repository's two own jobs, `release-smoke.yml` and `publish-crates.yml`, from
  `uses: ./.github/workflows/…` to `uses: $/.github/workflows/…`.

`$/` is GitHub's self-repository syntax (GitHub changelog, 2026-07-30), which the workflow
syntax reference now recommends for a reusable workflow in the same repository. dist adopted it
on purpose (cargo-dist PR 2507). The hand-pinned action commits and the repository's own jobs
are otherwise untouched. 0.34.0's other change, a newer default macOS runner, does not apply:
`dist-workspace.toml` names `macos-26`.

Risk: one downstream project reports that GitHub could not run a `$/` call and went back to
0.33.1; others run it. This repository's evidence comes in two parts:

- The pull request's own CI runs the release workflow in `upload` mode, which calls
  `release-smoke.yml` through `$/`. A green run proves the syntax here.
- `publish-crates.yml` runs only on a release tag, so its `$/` call, and crates.io trusted
  publishing behind it, is first exercised by the next release that is not a prerelease (a
  prerelease skips the job). By then the tag and the GitHub release exist, and the Homebrew job
  runs beside it, so the formula may be published too.
  - If GitHub cannot start the call at all, the tag is not moved or reused. cargo-dist goes back
    to 0.33.1, `release.yml` is regenerated, and a new release is made with a new version and
    tag, as `CONTRIBUTING.md` describes. Check which of the formula and the crates the failed
    release published first.
  - If the call starts and a step inside it fails for a passing reason, re-run the failed job:
    it skips crate versions already published.

If the pull request's release run fails on the `$/` call, cargo-dist stays at 0.33.1 in both
pins, `release.yml` stays as 0.33.1 generates it, and this spec records cargo-dist as held back.

actionlint 1.7.12 does not know `$/` and reports it. It is not part of CI.

### 4.3 rusqlite 0.43

Two minors. The bundled SQLite stays 3.54.1 (libsqlite3-sys 0.39.1 and 0.40.0 ship the same
one), so the database engine does not move. 0.42 and 0.43 fix use-after-free bugs in APIs
tagteam does not use (sessions, changesets, column metadata), and 0.43 rejects empty or
comment-only SQL. No tagteam statement is one: the suite passes unchanged.

### 4.4 jiff 1.0.3

jiff 1.0 is the same library as 0.2 with its API fixed. tagteam meets one of its breaking
changes: the type `Zoned` is now `ZonedDateTime`, and `Timestamp::to_zoned` and
`civil::Date::to_zoned` are now `to_zoned_datetime`. The change is that rename in nine files,
plus what rustfmt reflows around the longer name. tagteam uses none of the other changed APIs
(`Offset::saturating_add`, `Timestamp - Timestamp`, `Timestamp` to `SystemTime`, `ToSpan`,
`jiff_tzdb`): with the rename alone, every target compiles.

The published library crates (`tagteam-core`, `tagteam-engine`, `tagteam-tui`) take and return
jiff types, so their next release exposes jiff 1 types where it exposed jiff 0.2 ones: code that
builds against them with jiff 0.2 stops compiling until it moves to jiff 1. tagteam's versions
promise no compatibility between releases, and the workspace's own crates move together.

The pin stays exact, `=1.0.3`, with the same features (`std`, `tz-system`, `tzdb-zoneinfo`):
the token report's days and months must not move with a patch release. The suite's token-report
and time-zone tests are the check that they did not move with this one.

### 4.5 cargo-deny-action v2.1.5

Pinned at its commit, `bb119f768ea239e765aeaef5769558d4f21213aa` (the peeled tag, as the
existing pins are), with the `# v2.1.5` comment. It runs cargo-deny 0.22.2. Between v2.1.1 and
v2.1.5 the tool changed the default scope of `unmaintained` (tagteam's `deny.toml` sets it
explicitly), fixed `unsound` and `unmaintained` advisories being reported wrongly when a
workspace holds several versions of one crate, and added a `patched` diagnostic for an ignored
advisory whose fix its dependents could take. cargo-deny 0.22.2 passes on the new lockfile with
the unchanged `deny.toml`.

### 4.6 The lockfile

A plain `cargo update` with Cargo 1.100.1. Every package is then at the newest release that the
crates depending on it accept and that builds with Rust 1.89, and every crates.io dependency the
workspace names itself is at its newest release, except the one deliberate hold below.

- **Younger than seven days:** allocator-api2 0.2.22, anyhow 1.0.105, cc 1.12.0, clap and
  clap_builder 4.6.10, find-msvc-tools 0.1.16, hashbrown 0.17.2, litrs 1.1.0, pest and its three
  companions 2.9.7, sha2 0.11.2, toml_edit 0.25.21, toml_parser 1.1.7, zerocopy and
  zerocopy-derive 0.8.66.
- **Two crates join the build.** psm 0.1.32 (reached through pest's stack guard) now has two
  more build dependencies:
  - `ar_archive_writer` 0.5.5, https://crates.io/crates/ar_archive_writer, published 2026-11-11
    from `rust-lang/ar_archive_writer`, Apache-2.0 WITH LLVM-exception;
  - `object` 0.39.1, https://crates.io/crates/object, published 2026-04-21 from
    `gimli-rs/object`, Apache-2.0 OR MIT. 0.39 is what `ar_archive_writer` asks for; 0.41.0 is
    its newest.

  Both were looked up at crates.io and are the intended packages. They are used by psm's build
  script: neither is linked into tagteam.
- **Five crates leave the graph:** `atomic`, `bytemuck`, `defmt`, `defmt-macros`,
  `defmt-parser`.
- **Held on purpose:**
  - unicode-normalization stays `=0.1.25`. 0.1.26 moves to Unicode 18. The pin does not follow
    the crate: it follows the Unicode version of Claude Code's JavaScript runtime, whose NFC
    names the Keychain items tagteam must find. Moving it is a decision about Keychain names,
    not a dependency update.
- **Held by the crates that use them**, not by tagteam: generic-array 0.14.7, kem 0.3.0-pre.0
  and object 0.39.1, and the older release lines that some dependency still asks for beside the
  newest one (sha2 0.10, signal-hook 0.3, signal-hook-mio 0.2, thiserror 1, bitflags 1 and the
  like).
- ratatui `=0.30.2` and crossterm `=0.29.0` are already the newest.

### 4.7 `deny.toml`

Both ignores stay. With them removed, RUSTSEC-2023-0071 (`rsa` 0.9, through age's `ssh`
feature) and RUSTSEC-2026-0387 (crypto-bigint 0.5, through age's P-256 code) still fail the
check; age 0.12.1 is still its newest release, and the reasons recorded in `deny.toml` still
hold. Apart from those two, the advisory database matches no version the lockfile holds.

## 5. Out of scope

- A `[licenses]` or `[bans]` section in `deny.toml`. The file covers advisories only, by design.
- Unicode 18 for Keychain names (§4.6).

## 6. Verification

On the final commit, with Rust 1.100.1, except where a line names another moment:

- `cargo fmt --all --check`, and clippy with `-D warnings` in CI's three configurations;
- the full suite, `cargo test --workspace --locked --no-fail-fast --features
  tagteam/test-support`;
- `cargo check` with Rust 1.89 in the `msrv` job's four configurations;
- `cargo deny check advisories` with cargo-deny 0.22.2;
- freshness, checked once, in the same session that resolves the lockfile and against the
  registry as it is then; a release published after that is out of scope and does not reopen
  the check:
  - `cargo update --dry-run` reports nothing to update;
  - every crates.io dependency a workspace manifest names (the workspace's own path crates are
    not among them) is locked at the newest stable release crates.io lists, unicode-normalization
    excepted (where the lockfile holds two release lines of one, the line the workspace asks
    for);
- `Cargo.lock` names no package that `81507e2a`'s does not, except `ar_archive_writer` and
  `object`;
- the workflows use no external action that `81507e2a`'s do not, and every action commit in
  `release.yml` is one `dist-workspace.toml` pins;
- `dist generate --check` with the cargo-dist version both pins name;
- no build prints the `homepage` warning;
- no Rust file changes except by the jiff rename;
- CI green on the pull request, the release workflow's `release-smoke` call included.

## 7. Records

No CHANGELOG entry: tagteam behaves the same, and the minimum supported Rust is the same.
`README.md` and `CONTRIBUTING.md` name the new Rust and cargo-dist versions, and the
`Cargo.toml` comments name rusqlite 0.43 and keep jiff's reason for an exact pin.
