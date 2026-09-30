#!/usr/bin/env bash
# M2a Task 1 probe (spec Appendix A.5): records what Claude Code's profile, usage and token
# endpoints actually return, so tagteam's mock fixtures match reality.
#
# Safe against the real account: the profile and usage calls are read-only GETs with the live
# access token, as Claude Code makes them (the usage call spends one request of the hourly
# budget); the two token calls send a made-up refresh token, so no real refresh token is ever
# spent. No token is ever printed, logged or put on a command line: the access token reaches
# curl through `--config -` on stdin, and every body is redacted before it is written or shown.
set -euo pipefail

OUT=${TAGTEAM_PROBE_OUT:-crates/tagteam-cc/tests/fixtures/endpoints}
SECURITY=/usr/bin/security
PY=/usr/bin/python3
UA="tagteam/0.1.0"
CLIENT_ID=9d1c250a-e61b-44d9-88ed-5944d1962f5e
FAKE_CLIENT_ID=00000000-0000-4000-8000-0000000000ff
FAKE_RT=sk-ant-ort01-tagteam-probe-not-a-real-refresh-token
TOKEN_URL=https://platform.claude.com/v1/oauth/token
PROFILE_URL=https://api.anthropic.com/api/oauth/profile
USAGE_URL=https://api.anthropic.com/api/oauth/usage

WORK=$(mktemp -d "${TMPDIR:-/tmp}/tagteam-probe.XXXXXX")
chmod 700 "$WORK"
trap 'rm -rf "$WORK"' EXIT

acct() {
  local u="${USER:-}"
  [[ -n "$u" ]] || u=$(id -un 2>/dev/null || true)
  if [[ "$u" =~ ^[a-zA-Z0-9._-]+$ ]]; then printf '%s' "$u"; else printf 'claude-code-user'; fi
}
# Appendix A.2: CLAUDE_SECURESTORAGE_CONFIG_DIR (defined and non-empty), else CLAUDE_CONFIG_DIR,
# suffixes the item with the first 8 hex digits of its sha256; unset means the default item.
# TAGTEAM_PROBE_SERVICE overrides the whole name.
svc() {
  if [[ -n "${TAGTEAM_PROBE_SERVICE:-}" ]]; then printf '%s' "$TAGTEAM_PROBE_SERVICE"; return; fi
  local dir=""
  if [[ -n "${CLAUDE_SECURESTORAGE_CONFIG_DIR+x}" ]]; then
    dir="${CLAUDE_SECURESTORAGE_CONFIG_DIR}"
  else
    dir="${CLAUDE_CONFIG_DIR:-}"
  fi
  if [[ -z "$dir" ]]; then
    printf 'Claude Code-credentials'
  else
    printf 'Claude Code-credentials-%s' "$(printf '%s' "$dir" | shasum -a 256 | cut -c1-8)"
  fi
}

# The live credential JSON on stdout, captured by the caller into a variable, never shown.
live_credential() {
  if [[ "$(uname -s)" == Darwin ]]; then
    "$SECURITY" find-generic-password -a "$(acct)" -s "$(svc)" -w
  else
    cat "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/.credentials.json"
  fi
}

# The access token, from the credential JSON on stdin. `security -w` renders a secret with
# non-printable bytes as hex; Claude Code's JSON is printable, but decode hex just in case.
access_token() {
  "$PY" -c '
import json, sys
raw = sys.stdin.read().strip()
try:
    doc = json.loads(raw)
except ValueError:
    doc = json.loads(bytes.fromhex(raw).decode())
tok = (doc.get("claudeAiOauth") or {}).get("accessToken") or ""
if not tok:
    sys.exit("the live credential has no access token; log in with `claude` first")
sys.stdout.write(tok)
'
}

# Turns $WORK/{status,headers,body} into the redacted fixture envelope at $OUT/$1.json, and
# shows it. Redaction is deterministic and keeps every key, type and array length: emails,
# uuids, tokens and account/organization names become fixed fakes, numbered in order of first
# appearance, so two equal values stay equal.
record() {
  local name=$1
  mkdir -p "$OUT"
  "$PY" - "$WORK" "$OUT/$name.json" <<'PYEOF'
import json, re, sys
work, dest = sys.argv[1], sys.argv[2]
status = int(open(f"{work}/status").read().strip())
headers = {}
block = open(f"{work}/headers", encoding="latin-1").read().replace("\r\n", "\n").strip().split("\n\n")[-1]
for line in block.split("\n")[1:]:
    if ":" in line:
        k, v = line.split(":", 1)
        k = k.strip().lower()
        if k in ("content-type", "retry-after"):
            headers[k] = v.strip()
raw = open(f"{work}/body", "rb").read()
try:
    body = json.loads(raw)
except ValueError:
    body = raw.decode("utf-8", "replace")

EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
UUID = re.compile(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
NAME_KEYS = {"name", "full_name", "display_name", "organization_name"}
fakes = {"email": {}, "uuid": {}, "token": {}, "name": {}}
def fake(kind, value):
    table = fakes[kind]
    if value not in table:
        n = len(table) + 1
        table[value] = {
            "email": f"probe{n}@example.com",
            "uuid": f"00000000-0000-4000-8000-{n:012d}",
            "token": f"redacted-token-{n}",
            "name": f"Probe Name {n}",
        }[kind]
    return table[value]
def redact(v, key="", path=()):
    if isinstance(v, dict):
        return {k: redact(x, k, path + (k,)) for k, x in v.items()}
    if isinstance(v, list):
        return [redact(x, key, path) for x in v]
    if not isinstance(v, str):
        return v
    k = key.lower()
    if EMAIL.fullmatch(v) or "email" in k:
        return fake("email", v)
    owned = any(p in ("account", "organization", "user") for p in path)
    if UUID.fullmatch(v) or k.endswith("uuid") or (k == "id" and owned):
        return fake("uuid", v)
    if v.startswith("sk-ant-") or "token" in k or "secret" in k:
        return fake("token", v)
    if k in NAME_KEYS and owned:
        return fake("name", v)
    v = EMAIL.sub(lambda m: fake("email", m.group(0)), v)
    return UUID.sub(lambda m: fake("uuid", m.group(0)), v)

env = {"status": status, "headers": headers, "body": redact(body), "synthetic": False}
with open(dest, "w") as f:
    json.dump(env, f, indent=2)
    f.write("\n")
print(f"--- {dest}")
print(json.dumps(env, indent=2))
PYEOF
}

# curl with its secret-bearing options on stdin (`--config -`), never in argv.
curl_with() {
  local config=$1; shift
  printf '%s' "$config" | curl -sS --max-time 10 -o "$WORK/body" -D "$WORK/headers" \
    -w '%{http_code}' --config - "$@" > "$WORK/status"
}

profile() {
  local cred at
  cred=$(live_credential)
  at=$(printf '%s' "$cred" | access_token)
  curl_with "header = \"Authorization: Bearer $at\"
header = \"User-Agent: $UA\"
url = \"$PROFILE_URL\""
  record profile-200
}

usage() {
  local cred at
  cred=$(live_credential)
  at=$(printf '%s' "$cred" | access_token)
  curl_with "header = \"Authorization: Bearer $at\"
header = \"anthropic-beta: oauth-2025-04-20\"
header = \"User-Agent: $UA\"
url = \"$USAGE_URL\""
  record usage-200
}

token_post() {
  local client=$1 name=$2
  local body
  body=$(printf '{"grant_type":"refresh_token","refresh_token":"%s","client_id":"%s","scope":"user:inference user:profile"}' "$FAKE_RT" "$client")
  curl_with "header = \"Content-Type: application/json\"
header = \"User-Agent: $UA\"
url = \"$TOKEN_URL\"" --data-binary "$body"
  record "$name"
}

invalid_grant() { token_post "$CLIENT_ID" token-invalid-grant; }
invalid_client() { token_post "$FAKE_CLIENT_ID" token-invalid-client; }

# The decision gate (Appendix A.5): exits non-zero, naming what differs, when a recording's
# shape is not the one Tasks 7 and 8 parse.
check() {
  "$PY" - "$OUT" <<'PYEOF'
import json, sys
out = sys.argv[1]
def load(n):
    return json.load(open(f"{out}/{n}.json"))
problems = []
p = load("profile-200")
b = p["body"] if isinstance(p["body"], dict) else {}
if p["status"] != 200: problems.append(f"profile: status {p['status']}")
for path in (("account", "uuid"), ("account", "email")):
    cur = b
    for part in path:
        cur = cur.get(part) if isinstance(cur, dict) else None
    if not isinstance(cur, str) or not cur:
        problems.append(f"profile: no string at {'.'.join(path)}")
# Appendix A.6 treats a null or empty organization as '': a personal account has none.
org = b.get("organization")
if org is not None and not (isinstance(org, dict) and isinstance(org.get("uuid"), str)):
    problems.append("profile: organization is neither null nor an object with a string uuid")
u = load("usage-200")
if u["status"] != 200: problems.append(f"usage: status {u['status']}")
for n, want in (("token-invalid-grant", "invalid_grant"), ("token-invalid-client", "invalid_client")):
    t = load(n)
    err = t["body"].get("error") if isinstance(t["body"], dict) else None
    if t["status"] not in (400, 401, 403): problems.append(f"{n}: status {t['status']}")
    if err != want: problems.append(f"{n}: top-level error is {err!r}, expected {want!r}")
if problems:
    print("GATE FAILS:"); [print(" -", x) for x in problems]; sys.exit(1)
print("gate passes: every recorded shape matches Appendix A.5")
PYEOF
}

case "${1:-}" in
  profile|usage|invalid_grant|invalid_client|check) "$1" ;;
  all) profile; usage; invalid_grant; invalid_client; check ;;
  *) echo "usage: $0 all|profile|usage|invalid_grant|invalid_client|check" >&2; exit 2 ;;
esac
