#!/usr/bin/env bash
# R1 spike (spec §17): can `claude`, run from an SSH session, silently read a Keychain item
# that /usr/bin/security wrote? Uses a throwaway profile and a fake, never-expiring token, so
# no real login is touched and nothing is ever refreshed.
set -euo pipefail

PROFILE=/tmp/tagteam-r1-spike
SECURITY=/usr/bin/security
CRED='{"claudeAiOauth":{"accessToken":"sk-ant-oat01-r1-spike-not-a-real-token","refreshToken":"sk-ant-ort01-r1-spike-not-a-real-token","expiresAt":4102444800000,"scopes":["user:inference","user:profile"],"subscriptionType":"pro"}}'

acct() {
  local u="${USER:-}"
  if [[ "$u" =~ ^[a-zA-Z0-9._-]+$ ]]; then printf '%s' "$u"; else printf 'claude-code-user'; fi
}
svc() {
  printf 'Claude Code-credentials-%s' "$(printf '%s' "$PROFILE" | shasum -a 256 | cut -c1-8)"
}

keychain_state() {
  set +e; "$SECURITY" show-keychain-info 2>&1; echo "show-keychain-info rc=$?"; set -e
}
write() {
  mkdir -p "$PROFILE"; chmod 700 "$PROFILE"
  printf '%s\n' '{"oauthAccount":{"emailAddress":"r1-spike@example.com","organizationUuid":"","accountUuid":"00000000-0000-4000-8000-000000000001"},"hasCompletedOnboarding":true}' \
    > "$PROFILE/.claude.json"
  set +e
  "$SECURITY" add-generic-password -U -a "$(acct)" -s "$(svc)" \
    -X "$(printf '%s' "$CRED" | xxd -p | tr -d '\n')"
  echo "write: rc=$? service=$(svc) account=$(acct)"
  set -e
}
read_security() {
  set +e
  out=$("$SECURITY" find-generic-password -a "$(acct)" -w -s "$(svc)" 2>&1); rc=$?
  set -e
  if [ "$out" = "$CRED" ]; then m=yes; else m=no; fi
  echo "security read: rc=$rc bytes-match=$m"
}
probe() {
  set +e; "$SECURITY" find-generic-password -a "$(acct)" -s "$(svc)" >/dev/null 2>&1
  echo "existence probe (no -w): rc=$?"; set -e
}
read_claude() {
  local start=$SECONDS
  set +e
  out=$(CLAUDE_CONFIG_DIR="$PROFILE" claude auth status --json 2>&1); rc=$?
  set -e
  echo "claude auth status: rc=$rc elapsed=$((SECONDS - start))s"
  echo "$out"
}
cleanup() {
  "$SECURITY" delete-generic-password -a "$(acct)" -s "$(svc)" >/dev/null 2>&1 || true
  rm -rf "$PROFILE"; echo "cleaned up"
}
# The existence probe against an explicitly locked, throwaway keychain file (never the login
# keychain): answers what `find-generic-password` without -w returns when locked.
locked_probe() {
  local kc; kc="$(mktemp -d)/r1-locked.keychain"
  "$SECURITY" create-keychain -p r1 "$kc"
  "$SECURITY" add-generic-password -a probe -s tagteam-r1 -w x "$kc"
  "$SECURITY" lock-keychain "$kc"
  set +e
  "$SECURITY" show-keychain-info "$kc" >/dev/null 2>&1; echo "locked keychain info: rc=$?"
  "$SECURITY" find-generic-password -a probe -s tagteam-r1 "$kc" >/dev/null 2>&1; echo "probe, present item: rc=$?"
  "$SECURITY" find-generic-password -a missing -s tagteam-r1 "$kc" >/dev/null 2>&1; echo "probe, absent item: rc=$?"
  set -e
  "$SECURITY" delete-keychain "$kc"
}

case "${1:-}" in
  keychain_state|write|read_security|probe|read_claude|cleanup|locked_probe) "$1" ;;
  all) keychain_state; write; read_security; probe; read_claude ;;
  *) echo "usage: $0 all|keychain_state|write|read_security|probe|read_claude|cleanup|locked_probe" >&2; exit 2 ;;
esac
