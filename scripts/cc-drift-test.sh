#!/usr/bin/env bash
# Tests scripts/cc-drift.sh against fake claude builds: no real claude, no account, no network.
# Each fake answers `--version`, `auth status --json` and the probe as a logged-out claude of
# the tested version does, and carries the environment names a case needs in its own bytes.
#
# Usage: scripts/cc-drift-test.sh
# Exit 0: every case passed. 1: a case failed. Needs bash, jq and the tools cc-drift.sh needs.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
drift="$here/cc-drift.sh"
compat="$here/../crates/tagteam-cc/compat"
tested="$(grep -v '^#' "$compat/tested-cc-version" | grep -v '^[[:space:]]*$' | head -n1)"

work="$(mktemp -d "${TMPDIR:-/tmp}/cc-drift-test.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# Writes an executable fake claude to $1. The text in $2 follows the script's last line, where
# bash never reads it; printf escapes in it (\000 for NUL) are expanded.
fake_claude() {
  local path="$1" tail="$2"
  mkdir -p "$(dirname "$path")"
  cat >"$path" <<EOF
#!/usr/bin/env bash
case "\${1:-}" in
  --version) echo "$tested (Claude Code)" ;;
  auth)
    printf '{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty","analyticsDisabled":false,"projectsDirectory":"%s/.claude/projects","configDirectory":"%s/.claude"}\n' "\$HOME" "\$HOME"
    exit 1
    ;;
  -p)
    mkdir -p "\$HOME/.claude/projects/-fake"
    : >"\$HOME/.claude/projects/-fake/session.jsonl"
    exit 1
    ;;
  *) exit 2 ;;
esac
exit 0
EOF
  # shellcheck disable=SC2059 # the tail's escapes are meant
  printf "$tail" >>"$path"
  chmod 755 "$path"
}

# The names every native fake carries: scrubbed, known, unknown, and three that are not
# names on their own (a table byte after one, a prefix before one, a lower-case tail).
NATIVE='const a=process.env.CLAUDE_CODE_OAUTH_TOKEN,b=process.env.CLAUDE_CODE_SESSION_ID;\n'
NATIVE+='x("CLAUDE_CODE_GATEWAY_TOKEN_FILE_DESCRIPTOR","CLAUDE_CODE_TAGTEAM_FIXTURE_ONE");\n'
NATIVE+='y={ANTHROPIC_TAGTEAM_FIXTURE_TWO:1,CLAUDE_CODE_TAGTEAM_FIXTURE_THREE:2};\n'
NATIVE+='\000\020\000CLAUDE_CODE_TAGTEAM_FIXTURE_TABLE5\000\200\000\n'
NATIVE+='_CLAUDE_CODE_TAGTEAM_FIXTURE_PREFIXED ANT_CLAUDE_CODE_TAGTEAM_FIXTURE_ANT\n'
NATIVE+='CLAUDE_CODE_TAGTEAM_FIXTURE_LOWERx\n'
NATIVE_UNKNOWN='ANTHROPIC_TAGTEAM_FIXTURE_TWO CLAUDE_CODE_TAGTEAM_FIXTURE_ONE CLAUDE_CODE_TAGTEAM_FIXTURE_THREE'

failed=0
fail() {
  echo "FAIL $case_name: $*" >&2
  failed=1
}

# run_case NAME CLAUDE [VAR=VALUE...]: runs cc-drift.sh with the variables set, on CLAUDE, or
# on the claude its PATH finds when CLAUDE is empty, into $work/NAME.md. Sets rc, and names:
# the unclassified environment names the report lists, space-separated.
run_case() {
  case_name="$1"
  local claude="$2"
  shift 2
  report="$work/$case_name.md"
  local args=(--report "$report")
  [[ -z "$claude" ]] || args+=(--claude "$claude")
  rc=0
  env CC_DRIFT_PROBE_TIMEOUT=30 "$@" "$drift" "${args[@]}" >"$work/$case_name.out" 2>&1 || rc=$?
  names=""
  # shellcheck disable=SC2016 # the backticks are the report's, not a substitution
  [[ ! -f "$report" ]] || names="$(sed -n 's/^- `\([A-Z0-9_]*\)`$/\1/p' "$report" | tr '\n' ' ')"
  names="${names% }"
}

expect_rc() {
  [[ "$rc" == "$1" ]] || fail "exit $rc, expected $1; output:"$'\n'"$(cat "$work/$case_name.out")"
}

# 1. A native build: only the three unknown names are reported, sorted, and nothing else
#    drifts. Found on PATH, with the compat files the script finds by itself.
fake_claude "$work/native/bin/claude" "$NATIVE"
run_case native "" PATH="$work/native/bin:$PATH"
expect_rc 1
[[ "$names" == "$NATIVE_UNKNOWN" ]] || fail "names: '$names'"
grep -q '^### Unclassified environment names$' "$report" || fail "no environment section"
[[ "$(grep -c '^### ' "$report")" == 1 ]] || fail "another section drifted:"$'\n'"$(cat "$report")"
# shellcheck disable=SC2086 # split into one name per line
[[ "$(cat "$report.env-names.txt")" == "$(printf '%s\n' $NATIVE_UNKNOWN)" ]] ||
  fail "env-names.txt: $(cat "$report.env-names.txt")"

# 2. The same build once known-env classifies them, with a glob: no drift, and the
#    compat directory comes from CC_DRIFT_COMPAT_DIR.
cp -R "$compat" "$work/compat-classified"
printf '%s\n' 'known CLAUDE_CODE_TAGTEAM_FIXTURE_*' 'known ANTHROPIC_TAGTEAM_FIXTURE_TWO' \
  >>"$work/compat-classified/known-env"
run_case classified "$work/native/bin/claude" CC_DRIFT_COMPAT_DIR="$work/compat-classified"
expect_rc 0
[[ ! -s "$report" ]] || fail "report not empty:"$'\n'"$(cat "$report")"
[[ ! -e "$report.env-names.txt" ]] || fail "env-names.txt written without drift"

# 3. An npm install: claude is a link to the package's cli.js, and a name only another file
#    of the package holds is found too.
pkg="$work/npm/lib/node_modules/@anthropic-ai/claude-code"
fake_claude "$pkg/cli.js" "$NATIVE"
printf '{"name":"@anthropic-ai/claude-code","version":"%s"}\n' "$tested" >"$pkg/package.json"
mkdir -p "$pkg/vendor" "$work/npm/bin"
printf 'e.CLAUDE_CODE_TAGTEAM_FIXTURE_VENDOR\n' >"$pkg/vendor/extra.mjs"
ln -s ../lib/node_modules/@anthropic-ai/claude-code/cli.js "$work/npm/bin/claude"
run_case npm "" PATH="$work/npm/bin:$PATH"
expect_rc 1
[[ "$names" == "$NATIVE_UNKNOWN CLAUDE_CODE_TAGTEAM_FIXTURE_VENDOR" ]] || fail "names: '$names'"

# 4. More unknown names than the report shows: the first ones, a count of the rest, and all
#    of them beside the report.
run_case capped "$work/native/bin/claude" CC_DRIFT_ENV_REPORT_MAX=2
expect_rc 1
[[ "$names" == "ANTHROPIC_TAGTEAM_FIXTURE_TWO CLAUDE_CODE_TAGTEAM_FIXTURE_ONE" ]] || fail "names: '$names'"
grep -q '^- … and 1 more: all 3 are in ' "$report" || fail "no count of the rest:"$'\n'"$(cat "$report")"
[[ "$(wc -l <"$report.env-names.txt" | tr -d ' ')" == 3 ]] || fail "env-names.txt is not complete"

# 5. A build whose strings the extraction cannot read: no name run scrubs is found, so the
#    check proves nothing and the harness fails.
fake_claude "$work/blind/claude" 'x("CLAUDE_CODE_TAGTEAM_FIXTURE_ONE");\n'
run_case blind "$work/blind/claude"
expect_rc 2
grep -q 'found no name that run scrubs' "$work/blind.out" || fail "not the extraction's failure"

# 6. A known-env entry with a class the script does not know is a harness failure.
cp -R "$compat" "$work/compat-bad"
printf 'maybe CLAUDE_CODE_TAGTEAM_FIXTURE_ONE\n' >>"$work/compat-bad/known-env"
run_case bad-class "$work/native/bin/claude" CC_DRIFT_COMPAT_DIR="$work/compat-bad"
expect_rc 2
grep -q 'bad known-env entry' "$work/bad-class.out" || fail "not the known-env failure"

# 7. A malformed override is a harness failure, not drift (exit 1).
for bad in abc 0 -3; do
  run_case "bad-max$bad" "$work/native/bin/claude" CC_DRIFT_ENV_REPORT_MAX="$bad"
  expect_rc 2
  grep -q 'CC_DRIFT_ENV_REPORT_MAX must be a positive integer' "$work/$case_name.out" ||
    fail "not the override's failure"
done
run_case bad-timeout "$work/native/bin/claude" CC_DRIFT_PROBE_TIMEOUT=soon
expect_rc 2
grep -q 'CC_DRIFT_PROBE_TIMEOUT must be a positive integer' "$work/bad-timeout.out" ||
  fail "not the timeout's failure"

# 8. A known glob listed before the scrub block does not unscrub a name: scrub wins whatever
#    the order, so the build's only scrub name still counts and nothing drifts.
fake_claude "$work/oauth/claude" 'x("CLAUDE_CODE_OAUTH_TOKEN");\n'
cp -R "$compat" "$work/compat-order"
{
  echo 'known CLAUDE_CODE_OAUTH_*'
  cat "$compat/known-env"
} >"$work/compat-order/known-env"
run_case scrub-wins "$work/oauth/claude" CC_DRIFT_COMPAT_DIR="$work/compat-order"
expect_rc 0

if ((failed)); then
  exit 1
fi
echo "cc-drift-test: all cases passed"
