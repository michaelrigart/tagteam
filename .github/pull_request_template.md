## What

<!-- What this pull request changes. -->

## Why

<!-- The problem it solves, or the issue it closes (for example, "Closes #123"). -->

## How to verify

<!-- The commands you ran and what you saw. CI runs fmt, clippy -D warnings and the tests. -->

- [ ] `cargo fmt --all --check`, `cargo clippy` with `-D warnings` and `cargo test --workspace --features tagteam/test-support` pass
- [ ] Tests cover the change, and use the fake Keychain, not a real account
- [ ] `CHANGELOG.md` has an entry under `Unreleased`, if users can see the change
- [ ] No tokens, emails or account IDs in the code, fixtures, commits or this description
