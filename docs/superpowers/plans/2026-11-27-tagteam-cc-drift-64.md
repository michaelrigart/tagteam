# Claude Code 2.1.332 drift (issue #64): implementation plan

**Status:** In progress

**Goal:** `crates/tagteam-cc/compat/` matches Claude Code 2.1.332, the weekly drift check finds
nothing, and `tagteam run` scrubs `CLAUDE_CODE_RUNNER_MODEL_SOCKET`.

**Spec:** `docs/superpowers/specs/2026-09-26-tagteam-core-cli-design.md`, §12.5 "Environment" and
Appendix A.7, as amended on this branch (`e4af1d98`, signed 2026-11-27).

**Size:** about 5 lines of non-test code (one `CC_SCRUB` entry; the rest is compat data, tests
and docs), as the cloud-sessions token's commit `08c7c3af` was.

## Global Constraints

- The scrub list changes by exactly one name. Every other name 2.1.332 added stays `known`.
- `known-env`'s `scrub` set equals `CC_SCRUB` plus the descriptor glob (`compat.rs` pins it).
- No change to validation (§12.3) or to how `auth status` is parsed.

## Tasks

| Task | Depends on |
|---|---|
| 1 | none |
| 2 | 1 |
| 3 | 2 |

### Task 1: Scrub the runner's model socket

**Depends on:** none

- [ ] Test first: add `CLAUDE_CODE_RUNNER_MODEL_SOCKET` after `CLAUDE_CODE_SESSIONS_OAUTH_TOKEN`
  in `SCRUBBED` (`crates/tagteam-cc/tests/session.rs`, now 24 names), in the doctor test's set
  names and expected `unset` lines (`crates/tagteam-cc/tests/doctor.rs`), and in the scrub test
  of `crates/tagteam/src/app/context.rs`. Run them and see them fail.
- [ ] Add the name to `CC_SCRUB` (`crates/tagteam-cc/src/session.rs`), in §12.5's order, and a
  `scrub` line to `crates/tagteam-cc/compat/known-env` under "Carries or points at a credential",
  with 2.1.332 named in that comment.
- [ ] CHANGELOG, under Unreleased / Changed: `tagteam run` also removes the name, warns when it
  was set, and `tagteam doctor` reports it.
- [ ] The focused tests pass; `scripts/cc-drift.sh` reports no unclassified name for 2.1.332.

### Task 2: Bless 2.1.332

**Depends on:** 1

- [ ] `cargo xtask compat` against 2.1.332 exits 0 with no failed check.
- [ ] Set `crates/tagteam-cc/compat/tested-cc-version` and the README's version to 2.1.332, and
  add the CHANGELOG line, as `5957760a` did for 2.1.322.

### Task 3: Verification

**Depends on:** 2

- [ ] The full suite, clippy and rustfmt on the final commit; `scripts/cc-drift.sh` against the
  installed 2.1.332 exits 0 with an empty report; `scripts/cc-drift-test.sh` passes.
