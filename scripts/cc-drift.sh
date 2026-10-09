#!/usr/bin/env bash
# Account-free Claude Code compatibility checks (spec 15.4), run by the weekly
# cc-drift workflow. Everything runs in a throwaway HOME, so no account is involved
# and the caller's ~/.claude is never touched.
#
# Usage: scripts/cc-drift.sh [--claude PATH] --report FILE
#
# Writes a Markdown drift report to FILE, empty when there is no drift.
# Exit 0: no drift. 1: drift. 2: the harness failed (claude missing, jq missing, a probe
# that cannot be shown to have initialised ~/.claude, ...).
#
# Checks (independent, so one failing does not hide another):
#   a. `claude --version` has the known shape and is not newer than the tested version.
#   b. `claude auth status --json`, logged out, matches the recorded fixture.
#   c. Every top-level entry of ~/.claude, after one headless `claude -p` run with a
#      dummy API key, is on the known-shared or known-private list. The probe must have
#      created ~/.claude/projects/*, else the check proves nothing (harness failure).
#   d. Every CLAUDE_CODE_* and ANTHROPIC_* name in the claude executable (and, for an npm
#      install, in every file of its package) is matched by a known-env entry. A name counts
#      only where it stands alone between punctuation or whitespace, so a byte that a string
#      table packs after a name never makes a new one. The check must find a name that `run`
#      scrubs, else it proves nothing (harness failure). The report lists the first 150
#      unmatched names (override: CC_DRIFT_ENV_REPORT_MAX); FILE.env-names.txt lists them all.
#
# Data files live in crates/tagteam-cc/compat/ (override: CC_DRIFT_COMPAT_DIR).
# The probe timeout in seconds defaults to 300 (override: CC_DRIFT_PROBE_TIMEOUT): claude
# retries the rejected dummy key for about three minutes before it exits.

set -euo pipefail

usage() {
  echo "usage: $0 [--claude PATH] --report FILE" >&2
  exit 2
}

die() {
  echo "cc-drift: $*" >&2
  exit 2
}

claude_arg=""
report=""
while (($#)); do
  case "$1" in
    --claude)
      (($# >= 2)) || usage
      claude_arg="$2"
      shift 2
      ;;
    --report)
      (($# >= 2)) || usage
      report="$2"
      shift 2
      ;;
    *) usage ;;
  esac
done
[[ -n "$report" ]] || usage

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
compat="${CC_DRIFT_COMPAT_DIR:-$script_dir/../crates/tagteam-cc/compat}"
probe_timeout="${CC_DRIFT_PROBE_TIMEOUT:-300}"
env_report_max="${CC_DRIFT_ENV_REPORT_MAX:-150}"

command -v jq >/dev/null || die "jq not found"
for f in tested-cc-version known-shared known-private known-env auth-status-logged-out.json; do
  [[ -r "$compat/$f" ]] || die "missing compat file: $compat/$f"
done

if [[ -n "$claude_arg" ]]; then
  claude_bin="$claude_arg"
else
  claude_bin="$(command -v claude)" || die "claude not found on PATH (use --claude PATH)"
fi
[[ -x "$claude_bin" ]] || die "claude is not executable: $claude_bin"
case "$claude_bin" in
  /*) ;;
  *) claude_bin="$PWD/$claude_bin" ;;
esac

: >"$report"
rm -f "$report.env-names.txt"

# Env overrides that would point claude at a real account, config or provider.
while IFS= read -r name; do
  case "$name" in
    CLAUDE* | ANTHROPIC_* | XDG_CONFIG_HOME) unset "$name" ;;
  esac
done < <(compgen -v)

work="$(mktemp -d "${TMPDIR:-/tmp}/cc-drift.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir "$work/home" "$work/cwd"
HOME="$(cd "$work/home" && pwd -P)"
export HOME
cwd="$(cd "$work/cwd" && pwd -P)"

# Portable timeout (macOS has none): run "$@" with stdout to FILE, stderr to FILE.err and
# stdin closed; kill its process group after SECS. Sets run_rc (124 on timeout).
run_rc=0
run_limited() {
  local secs="$1" out="$2" pid elapsed=0
  shift 2
  set -m
  "$@" </dev/null >"$out" 2>"$out.err" &
  pid=$!
  set +m
  while kill -0 "$pid" 2>/dev/null; do
    if ((elapsed >= secs)); then
      kill -TERM -- "-$pid" 2>/dev/null || true
      sleep 2
      kill -KILL -- "-$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
      run_rc=124
      return 0
    fi
    sleep 1
    elapsed=$((elapsed + 1))
  done
  run_rc=0
  wait "$pid" || run_rc=$?
}

read_list() {
  # Entries of a compat list: no comments, no blank lines, no CR or edge spaces.
  local line
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line%$'\r'}"
    line="${line#"${line%%[![:space:]]*}"}"
    line="${line%"${line##*[![:space:]]}"}"
    [[ -z "$line" || "$line" == \#* ]] && continue
    printf '%s\n' "$line"
  done <"$1"
}

drift=0
sections=""
add_section() {
  drift=1
  sections+=$'\n'"### $1"$'\n\n'"$2"$'\n'
}

# a. Version ------------------------------------------------------------------
ver_out="$work/version.out"
run_limited 30 "$ver_out" "$claude_bin" --version
((run_rc == 0)) || die "'$claude_bin --version' failed (exit $run_rc)"
version_line="$(head -n1 "$ver_out" | tr -d '\r')"

tested="$(read_list "$compat/tested-cc-version" | head -n1)"
[[ "$tested" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "bad tested-cc-version: $tested"

if [[ "$version_line" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)\ \(Claude\ Code\)$ ]]; then
  IFS=. read -r t1 t2 t3 <<<"$tested"
  newer=0
  a=("${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}")
  b=("$t1" "$t2" "$t3")
  for i in 0 1 2; do
    if ((10#${a[i]} > 10#${b[i]})); then
      newer=1
      break
    elif ((10#${a[i]} < 10#${b[i]})); then
      break
    fi
  done
  if ((newer)); then
    add_section "Version" "\`claude --version\` reports \`$version_line\`, newer than the tested \`$tested\`."
  fi
else
  add_section "Version" "\`claude --version\` output no longer matches \`^[0-9]+\.[0-9]+\.[0-9]+ \(Claude Code\)\$\`: \`$version_line\`."
fi

# b. auth status --json, logged out -------------------------------------------
auth_out="$work/auth.out"
run_limited 30 "$auth_out" "$claude_bin" auth status --json
auth_rc=$run_rc
auth_problems=""
add_problem() { auth_problems+="- $1"$'\n'; }

if ((auth_rc == 124)); then
  add_problem "\`claude auth status --json\` did not exit within 30 s."
else
  ((auth_rc == 1)) || add_problem "exit code: expected \`1\`, actual \`$auth_rc\`."
  if jq -e 'type == "object"' "$auth_out" >/dev/null 2>&1; then
    # Paths are reported relative to the throwaway HOME, so the report is stable.
    actual="$(jq -c --arg home "$HOME" \
      'with_entries(.value |= if type == "string" then split($home) | join("<HOME>") else . end)' \
      "$auth_out")"
    problems="$(jq -r --argjson actual "$actual" '
      . as $want
      | ( ($want | keys) - ($actual | keys) | map("missing key `\(.)`") ),
        ( ($actual | keys) - ($want | keys) | map("unexpected key `\(.)`") ),
        ( $want | keys[] | select(. as $k | $actual | has($k))
          | select(($want[.] | type) != ($actual[.] | type))
          | "type of `\(.)`: expected `\($want[.] | type)`, actual `\($actual[.] | type)`" ),
        ( ["loggedIn", "authMethod", "configDirectory", "projectsDirectory"][]
          | select($actual[.] != $want[.])
          | "value of `\(.)`: expected `\($want[.] | tojson)`, actual `\($actual[.] | tojson)`" )
      | if type == "array" then .[] else . end
      | "- " + .
    ' "$compat/auth-status-logged-out.json")"
    [[ -z "$problems" ]] || auth_problems+="$problems"$'\n'
  else
    add_problem "stdout is not a JSON object."
  fi
fi
if [[ -n "$auth_problems" ]]; then
  add_section "\`auth status --json\` (logged out)" "${auth_problems%$'\n'}"
fi

# c. Top-level ~/.claude entries ----------------------------------------------
# A logged-out claude creates almost nothing, so run one headless prompt with a
# dummy key first; the API rejects it and no account is involved.
probe_out="$work/probe.out"
probe_rc=0
(
  cd "$cwd"
  export ANTHROPIC_API_KEY="sk-ant-api03-cc-drift-dummy-not-a-key"
  run_limited "$probe_timeout" "$probe_out" \
    "$claude_bin" -p "Reply with the single word ok." --max-turns 1
  exit "$run_rc"
) || probe_rc=$?
echo "cc-drift: probe finished (exit $probe_rc)" >&2

# The API rejecting the dummy key (a non-zero exit) is expected, but a probe that never
# initialised ~/.claude would make the entry check below pass vacuously.
((probe_rc != 124)) || die "the probe did not finish within ${probe_timeout} s"
if ! [[ -d "$HOME/.claude/projects" ]] || [[ -z "$(ls -A "$HOME/.claude/projects")" ]]; then
  die "the probe (exit $probe_rc) did not initialise ~/.claude; stderr tail:"$'\n'"$(tail -n 5 "$probe_out.err")"
fi

known=()
while IFS= read -r line; do known+=("$line"); done < <(
  read_list "$compat/known-shared"
  read_list "$compat/known-private"
)

unknown=""
if [[ -d "$HOME/.claude" ]]; then
  shopt -s dotglob nullglob
  for path in "$HOME/.claude"/*; do
    name="${path##*/}"
    ok=0
    for pat in "${known[@]}"; do
      # shellcheck disable=SC2053 # the list entry is a glob pattern on purpose
      if [[ "$name" == $pat ]]; then
        ok=1
        break
      fi
    done
    ((ok)) && continue
    if [[ -L "$path" ]]; then
      kind=symlink
    elif [[ -d "$path" ]]; then
      kind=dir
    elif [[ -f "$path" ]]; then
      kind="file"
    else
      kind=other
    fi
    unknown+="- \`$name\` ($kind)"$'\n'
  done
  shopt -u dotglob nullglob
fi
if [[ -n "$unknown" ]]; then
  add_section "Unknown top-level \`~/.claude\` entries" \
    "On neither the known-shared nor the known-private list:"$'\n\n'"${unknown%$'\n'}"
fi

# d. Environment names ---------------------------------------------------------
# Follows a symlink chain to the file it names (no `readlink -f` on older macOS).
resolve_link() {
  local p="$1" target hops=0
  while [[ -L "$p" ]]; do
    hops=$((hops + 1))
    ((hops <= 40)) || die "too many symlinks resolving $1"
    target="$(readlink "$p")"
    case "$target" in
      /*) p="$target" ;;
      *) p="$(dirname "$p")/$target" ;;
    esac
  done
  printf '%s\n' "$p"
}

env_classes=()
env_patterns=()
while IFS= read -r line; do
  class="${line%%[[:space:]]*}"
  pattern="${line#"$class"}"
  pattern="${pattern#"${pattern%%[![:space:]]*}"}"
  case "$class" in
    scrub | known) ;;
    *) die "bad known-env entry (the class is scrub or known): $line" ;;
  esac
  [[ "$pattern" =~ ^[A-Z0-9_*]+$ ]] || die "bad known-env entry (the name): $line"
  env_classes+=("$class")
  env_patterns+=("$pattern")
done < <(read_list "$compat/known-env")
((${#env_patterns[@]} > 0)) || die "known-env lists no name"

binary="$(resolve_link "$claude_bin")"
env_sources=("$binary")
dir="$(dirname "$binary")"
for _ in 1 2 3; do
  if [[ -f "$dir/package.json" ]] &&
    [[ "$(jq -r '.name // empty' "$dir/package.json" 2>/dev/null)" == "@anthropic-ai/claude-code" ]]; then
    while IFS= read -r -d '' f; do
      [[ "$f" == "$binary" ]] || env_sources+=("$f")
    done < <(find "$dir" -type f -print0)
    break
  fi
  [[ "$dir" == / ]] && break
  dir="$(dirname "$dir")"
done

# Bytes other than printable ASCII, tab, LF and CR become '~'; every byte but a letter, digit,
# '_' or '~' then ends a token; a token is a name only when the whole of it is one.
env_names="$work/env-names"
for src in "${env_sources[@]}"; do
  LC_ALL=C tr '\000-\010\013\014\016-\037\177-\377' '~' <"$src"
  echo
done | LC_ALL=C tr -cs 'A-Za-z0-9_~' '\n' |
  { LC_ALL=C grep -E '^(CLAUDE_CODE|ANTHROPIC)_[A-Z0-9_]+$' || true; } |
  LC_ALL=C sort -u >"$env_names"

unknown_env=""
unknown_count=0
scrub_seen=0
while IFS= read -r name; do
  matched=""
  for i in "${!env_patterns[@]}"; do
    # shellcheck disable=SC2053 # the entry is a glob pattern on purpose
    if [[ "$name" == ${env_patterns[i]} ]]; then
      matched="${env_classes[i]}"
      break
    fi
  done
  case "$matched" in
    scrub) scrub_seen=1 ;;
    known) ;;
    *)
      unknown_env+="$name"$'\n'
      unknown_count=$((unknown_count + 1))
      ;;
  esac
done <"$env_names"
((scrub_seen)) ||
  die "found no name that run scrubs in $binary (${#env_sources[@]} file(s) read); the extraction cannot read this build"
if ((unknown_count)); then
  printf '%s' "$unknown_env" >"$report.env-names.txt"
  shown="$(printf '%s' "$unknown_env" | sed -n "1,${env_report_max}{s/^/- \`/;s/\$/\`/;p;}")"
  if ((unknown_count > env_report_max)); then
    shown+=$'\n'"- … and $((unknown_count - env_report_max)) more: all $unknown_count are in the run's \`env-names.txt\` artifact (locally, \`FILE.env-names.txt\`)."
  fi
  add_section "Unclassified environment names" \
    "$unknown_count \`CLAUDE_CODE_*\` or \`ANTHROPIC_*\` name(s) in the binary that no \`known-env\` entry matches:"$'\n\n'"$shown"
fi

# Report ----------------------------------------------------------------------
if ((drift)); then
  {
    echo "Claude Code \`$version_line\` on \`$(uname -s)\`"
    printf '%s' "$sections"
  } >"$report"
  exit 1
fi
exit 0
