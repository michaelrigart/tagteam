# Contributing to tagteam

Thank you for helping. This page covers how to build and test tagteam, how its design records
work, and what a pull request needs. For anything larger than a small fix, please open an issue
first so we can agree on the approach before you write code.

By contributing, you agree that your contribution is licensed under the [MIT license](LICENSE).

## Toolchain

- **Rust.** `rust-toolchain.toml` pins Rust 1.88.0 with `rustfmt` and `clippy`. With
  [rustup](https://rustup.rs) installed, cargo picks it up on first use inside the checkout. The
  workspace declares a minimum supported Rust version of 1.85 (edition 2024).
- **mise.** `mise.toml` pins only `cargo-dist` 0.33.0, which builds releases. You do not need it
  for ordinary development.
- **C compiler.** SQLite is bundled and compiled from source.

The workspace has these crates under `crates/`:

| Crate | Role |
|---|---|
| `tagteam` | The CLI binary |
| `tagteam-core` | Provider-neutral domain types and policy, including the auto-switch decision |
| `tagteam-provider` | The `Provider` trait and the I/O primitives providers share (Keychain, HTTP, locks, processes) |
| `tagteam-cc` | The Claude Code provider |
| `tagteam-engine` | The store, the vault and the provider-generic operations (switching, refresh, auto-switch, sessions) |
| `tagteam-fake` | A test-only second provider that keeps the `Provider` trait provider-neutral |
| `xtask` | Development tasks (`cargo xtask compat`) |

## Build and test

```sh
cargo build -p tagteam
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --locked --features tagteam/test-support -- -D warnings
cargo test --workspace --locked --no-fail-fast --features tagteam/test-support
```

These are the checks CI runs on macOS and Linux (`.github/workflows/ci.yml`), and a pull request
needs all of them to pass. The `test-support` feature gives the tests a file-backed fake
Keychain, crash points and an injectable lock timeout. It is never part of a release build.

On macOS, CI also runs the tests that use the real `/usr/bin/security` against a throwaway
keychain file (never your login keychain):

```sh
cargo test -p tagteam-provider --locked --features real_keychain --test real_keychain
```

`tagteam-engine` has a further real-Keychain test that is marked `#[ignore]`:

```sh
cargo test -p tagteam-engine --features real_keychain --test run_real_keychain -- --ignored
```

### Test quirks

- **Pty tests need a real terminal environment.** `tagteam`'s `prompt::tests` and
  `tests/signals.rs` open a pseudo-terminal. They fail with `openpty: Operation not permitted`
  inside a sandbox that denies it, such as an agent or container sandbox. Run them from an
  ordinary, unsandboxed terminal:

  ```sh
  cargo test -p tagteam --features tagteam/test-support --lib prompt::tests
  cargo test -p tagteam --features tagteam/test-support --test signals
  ```

- **`real_keychain` tests need an unsandboxed terminal** on macOS, because they run
  `/usr/bin/security` and create a keychain file.
- **Linux tests run in Docker.** To reproduce the Linux CI run from a Mac, run the tests in a
  `rust:1.88` container. They must run as a non-root user (root defeats the permission-based
  fault injection in the engine tests), as a user with a passwd entry, and with `HOME` outside
  `/tmp`:

  ```sh
  docker run --rm -v "$PWD":/src:ro \
    -v tagteam-target:/target -v tagteam-registry:/usr/local/cargo/registry \
    -e CARGO_TARGET_DIR=/target -e RUSTUP_TOOLCHAIN=1.88.0 -w /src rust:1.88 \
    bash -c 'useradd -m -u 1000 tester && chown -R tester /target /usr/local/cargo &&
      setpriv --reuid=1000 --regid=1000 --clear-groups \
        env HOME=/home/tester USER=tester \
        cargo test --workspace --locked --no-fail-fast --features tagteam/test-support'
  ```

- **New dependencies.** If a sandbox stops cargo writing to `~/.cargo/registry`, run
  `cargo fetch --locked` once outside it, then build as usual.

## Claude Code compatibility

tagteam depends on Claude Code internals that are not a public interface. The facts it keeps
about them are in `crates/tagteam-cc/compat/`, and `compat/tested-cc-version` records the newest
Claude Code release tagteam has been checked against. A weekly CI job (`cc-drift.yml`) installs
the latest `claude` and opens an issue when something drifts.

`cargo xtask compat` runs the full set of interop checks locally, against the real `claude`
and a **dedicated test account**. Do not use an account you rely on: the checks refresh that
account's tokens and spend a few requests of its usage.

```sh
cargo xtask compat login     # log the test account in once
cargo xtask compat           # run every check; reports go to target/compat/
cargo xtask compat --bless   # after a full pass, record claude's version as tested
```

Every Claude Code home it touches is a scratch directory, and it refuses to start if a
Keychain service name it would touch is not one of those. `cargo xtask compat --help` lists
the other options. Only a maintainer needs to run it, and only `--bless` changes
`tested-cc-version`.

## Design records

tagteam's design lives in the repository, next to the code, under `docs/superpowers/`:

| Path | Holds | Tracked |
|---|---|---|
| `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md` | The approved design | yes |
| `docs/superpowers/plans/YYYY-MM-DD-<topic>.md` | The task-by-task plan | yes |
| `.superpowers/`, `docs/superpowers/runs/` | Execution state and notes | never (gitignored) |

- Specs and plans are **permanent**. They are versioned with the code, never deleted, pruned or
  gitignored, and not rewritten after the fact. The exception is material that must go for
  security or legal reasons, such as a secret or personal data.
- Each record carries a `**Status:**` line with exactly one of: `Approved`, `In progress`,
  `Implemented`, `Superseded — see <link>`, or `Abandoned — <reason>`. Set `In progress` while
  the work is under review, and `Implemented` in the final commit before merge, citing the pull
  request.
- A spec is committed when it is signed off, and its plan before the work starts. Code lands in
  task-sized commits after that.
- The core design is `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`. Section
  numbers in code comments and error messages (such as "section 9.4") refer to it. When a change
  alters behaviour the spec describes, update the spec in the same pull request or open an
  issue to discuss a new one.

You do not need to write a spec for a bug fix or a small change.

## Releasing

- Set the version to the UTC date and time of the release, `YYYYMMDD.HHMM.0`, with no leading
  zeros in `HHMM` (09:05 is `905`). Change `version` in `[workspace.package]` and the four
  internal `version = "…"` entries in `[workspace.dependencies]` (`tagteam-core`,
  `tagteam-provider`, `tagteam-cc` and `tagteam-engine`) together, then refresh `Cargo.lock`.
- Add a dated section to the [changelog](CHANGELOG.md), with the version as its heading.
- Tag the commit `v<version>` (for example `v20261013.2030.0`) and push the tag. The release
  publishes the GitHub archives, the Homebrew formula and the crates on crates.io. Prereleases
  publish none of the last two.
- A failed crates.io run can be re-run: the job skips every crate whose version is already
  published and carries on with the rest. crates.io rate-limits new crate names, so if the
  first release of a new crate is refused with HTTP 429, wait ten minutes and re-run.

## Pull requests

- **Open an issue first** for anything that changes behaviour, adds a command or touches the
  design. Small fixes and documentation changes can go straight to a pull request.
- **Keep it focused.** One concern per pull request, in small commits with imperative-mood
  messages (for example, "Refuse an empty alias"). Match the surrounding code and style.
- **CI must be green.** That means `cargo fmt`, `clippy` with `-D warnings` (both with and
  without `tagteam/test-support`), and the tests, on macOS and Linux.
- **Add tests** for new behaviour and for each bug you fix. Tests must use the fake Keychain,
  scratch directories and `tagteam-fake`; they must never touch a real account, your login
  keychain or your real Claude Code home.
- **Update the [changelog](CHANGELOG.md)** under `Unreleased` for any change a user can see.
- **Never include secrets.** That covers tokens, API keys, emails, account IDs and the output of
  `tagteam doctor` or `list` from a real machine, in code, fixtures, commit messages and
  comments. Use obvious placeholders such as `user@example.com`.
- **Do not hand-edit `.github/workflows/release.yml`.** `cargo-dist` generates it from
  `dist-workspace.toml`. Change the dist configuration and regenerate the file with
  `dist generate` instead.
- Fill in the pull request template. Say what changed, why, and how you verified it.

To report a security problem, follow [SECURITY.md](SECURITY.md) rather than opening a public
issue.
