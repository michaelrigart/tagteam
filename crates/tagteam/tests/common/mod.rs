//! Shared by the in-process tests (`app.rs`) and the binary tests (`cli.rs`, `kill.rs`). Each
//! test file is its own crate and uses only part of this module, so an item unused by one of
//! them is not dead code overall.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use serde_json::{Value, json};
use tagteam_cc::live::Platform;
use tagteam_cc::{ClaudeCode, ItemKind, keychain_account, keychain_service};
use tagteam_core::{AccountId, CLAUDE_CODE, PollBudget, PollPlan, ProviderId, Window, WindowKind};
use tagteam_engine::store::{Eligibility, Reserve, Store};
use tagteam_engine::vault::SERVICE;
use tagteam_provider::atomic::ensure_private_dir;
use tagteam_provider::splice::replace_top_level;
use tagteam_provider::{
    Env, FakeKeychain, FileKeychain, FlockGuard, Keychain, LAUNCH_DIR, ProfileMarker, Provider,
    canonical_profile_path, profile_path,
};

/// Appendix A.3's refusal, pinned verbatim: its wording is part of the user-facing contract.
pub const LOCKED: &str = "the login keychain is locked (common over SSH); run `security unlock-keychain ~/Library/Keychains/login.keychain-db`, then retry";

/// Where every endpoint points unless a test starts a `MockServer`: a local port nothing
/// listens on, so a request fails at once as `PreSend` and no test reaches the network.
pub const OFFLINE_API_BASE: &str = "http://127.0.0.1:9";

/// Now, in epoch seconds.
pub fn now_epoch_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// The binary in an isolated environment rooted at `root`, with its file-backed Keychain.
pub fn std_cmd(root: &Path) -> std::process::Command {
    let mut c = std::process::Command::new(assert_cmd::cargo::cargo_bin("tagteam"));
    c.env_clear()
        .env("HOME", root.join("home"))
        .env("USER", "tester")
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("TAGTEAM_TEST_KEYCHAIN_DIR", root.join("keychain"))
        .env("TAGTEAM_TEST_PLATFORM", "macos")
        .env("TAGTEAM_TEST_API_BASE", OFFLINE_API_BASE);
    c
}

pub fn cmd(root: &Path) -> Command {
    Command::from_std(std_cmd(root))
}

/// A home where Claude Code has run, with nobody logged in.
pub fn seed_home(env: &Env) {
    fs::create_dir_all(env.home.join(".claude")).unwrap();
    fs::write(env.home.join(".claude.json"), "{\n  \"userID\": \"u\"\n}\n").unwrap();
}

/// What `claude /login` leaves behind, for a login in organization `org` ("" is personal): its
/// `oauthAccount`, and its credential in the Keychain item Claude Code reads. Logging in again
/// with the same identity and a new `rt` is Claude Code rotating the credential.
pub fn login(env: &Env, kc: &dyn Keychain, email: &str, org: &str, rt: &str) {
    let path = env.home.join(".claude.json");
    let doc = fs::read(&path).unwrap();
    let acct = json!({"emailAddress": email, "organizationUuid": org, "accountUuid": format!("uuid-{email}-{org}")});
    fs::write(
        &path,
        replace_top_level(&doc, "oauthAccount", &acct).unwrap(),
    )
    .unwrap();
    let cred = json!({"claudeAiOauth": {"accessToken": "at", "refreshToken": rt, "refreshTokenExpiresAt": 1_797_000_000_000i64}});
    kc.upsert(
        &keychain_service(env, ItemKind::OAuth),
        &keychain_account(env),
        cred.to_string().as_bytes(),
    )
    .unwrap();
}

/// `a@x.co` at position 1 and `b@x.co` at position 2 (live), both added through the binary with
/// every endpoint offline (`std_cmd`'s default).
fn add_two_accounts(root: &Path) {
    let env = Env::for_test(root);
    let kc = FileKeychain::new(root.join("keychain"));
    seed_home(&env);
    login(&env, &kc, "a@x.co", "", "rt-a");
    cmd(root).arg("add").assert().success();
    login(&env, &kc, "b@x.co", "", "rt-b");
    cmd(root).arg("add").assert().success();
}

/// The two accounts of `add_two_accounts`, with their ids read from `list`.
pub fn two_accounts(root: &Path) -> (String, String) {
    add_two_accounts(root);
    let out = cmd(root).args(["list", "--json"]).output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let id = |i: usize| v["accounts"][i]["id"].as_str().unwrap().to_owned();
    (id(0), id(1))
}

/// The accounts of `two_accounts`, with the ids read from the store rather than from `list`.
/// `list` collects usage, and with every endpoint offline it would record a failed fetch, and
/// its backoff, before the test seeds its readings.
pub fn two_fresh_accounts(root: &Path) -> (String, String) {
    add_two_accounts(root);
    let store = Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let rows = store.accounts(&ProviderId::new(CLAUDE_CODE)).unwrap();
    (
        rows[0].id.as_str().to_owned(),
        rows[1].id.as_str().to_owned(),
    )
}

/// A window as a provider reports it, without provider detail.
pub fn usage_window(
    key: &str,
    label: &str,
    kind: WindowKind,
    pct: f64,
    resets_at: Option<i64>,
    period_s: Option<i64>,
) -> Window {
    Window {
        key: key.to_owned(),
        label: label.to_owned(),
        kind,
        pct,
        resets_at,
        period_s,
        detail: None,
    }
}

/// Records `windows` as account `id`'s reading taken at `at_s`, through the store's own write
/// path, as a fetch would: reserve (§8.3 phase 1), then record (phase 3), which writes
/// `usage_state` and one `usage_samples` row per window. One account's readings go in time
/// order, at least 180 s apart (the on-demand rule's minimum age).
pub fn record_reading(root: &Path, id: &str, at_s: i64, windows: &[Window]) {
    let store = Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let row = store.account(&AccountId::from_string(id)).unwrap().unwrap();
    let reservation = match store
        .reserve_usage(
            &row,
            at_s * 1000,
            Eligibility::OnDemand,
            &PollBudget::STANDARD,
        )
        .unwrap()
    {
        Reserve::Reserved(r) => r,
        other => panic!("no reservation for a reading at {at_s}: {other:?}"),
    };
    let plan = PollPlan {
        interval_s: 300,
        next_poll_at: at_s + 300,
    };
    assert!(
        store
            .record_usage(&reservation, windows, at_s, &plan, 180)
            .unwrap(),
        "the record was fenced out"
    );
}

/// Records `readings` readings of account `id`, `spacing_s` apart, the last at `last_s`, each
/// with the windows `windows_at(taken_at)` gives: a long history as fetches would have left it,
/// through the store's own write path on one connection. `spacing_s` must keep inside the hourly
/// budget (§8.6: 20 requests in 3 660 s, so at least 183 s) and the readings in time order.
pub fn record_history(
    root: &Path,
    id: &str,
    last_s: i64,
    readings: usize,
    spacing_s: i64,
    windows_at: &dyn Fn(i64) -> Vec<Window>,
) {
    let store = Store::open_existing(&Env::for_test(root).data_dir().join("tagteam.db"))
        .unwrap()
        .unwrap();
    let row = store.account(&AccountId::from_string(id)).unwrap().unwrap();
    for i in (0..readings).rev() {
        let at_s = last_s - i as i64 * spacing_s;
        let Reserve::Reserved(reservation) = store
            .reserve_usage(
                &row,
                at_s * 1000,
                Eligibility::Scheduled,
                &PollBudget::STANDARD,
            )
            .unwrap()
        else {
            panic!("no reservation for a reading at {at_s}");
        };
        let plan = PollPlan {
            interval_s: spacing_s,
            next_poll_at: at_s + spacing_s,
        };
        assert!(
            store
                .record_usage(&reservation, &windows_at(at_s), at_s, &plan, 180)
                .unwrap(),
            "the record was fenced out"
        );
    }
}

/// Pads `~/.claude.json` with Claude Code's per-project state to about `bytes` bytes, as a
/// machine that has run it for months has: what `statusline` must parse on a cache miss.
pub fn bloat_claude_json(root: &Path, bytes: usize) {
    let path = Env::for_test(root).home.join(".claude.json");
    let entry = |n: usize| {
        json!({
            "allowedTools": [],
            "history": (0..8).map(|h| json!({
                "display": format!("a prompt typed in project {n}, number {h}, long enough to look real"),
                "pastedContents": {},
            })).collect::<Vec<_>>(),
            "mcpServers": {},
            "lastCost": 1.25,
            "lastSessionId": format!("00000000-0000-4000-8000-{n:012}"),
        })
    };
    let each = serde_json::to_vec(&entry(0)).unwrap().len() + 40;
    let projects: serde_json::Map<String, Value> = (0..bytes / each + 1)
        .map(|n| (format!("/Users/tester/Code/project-{n}"), entry(n)))
        .collect();
    let doc = fs::read(&path).unwrap();
    fs::write(
        &path,
        replace_top_level(&doc, "projects", &Value::Object(projects)).unwrap(),
    )
    .unwrap();
}

/// Rewrites account `id`'s vault copy so its access token expires `in_ms` from now: inside
/// the 10-minute freshen window (§7.2) when `in_ms` is below 600 000.
pub fn expire_vault(root: &Path, id: &str, in_ms: i64) {
    let kc = FileKeychain::new(root.join("keychain"));
    let mut v: Value = serde_json::from_slice(&kc.find(SERVICE, id).present().unwrap()).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    v["claudeAiOauth"]["expiresAt"] = json!(now + in_ms);
    kc.upsert(SERVICE, id, v.to_string().as_bytes()).unwrap();
}

/// The email of the `oauthAccount` Claude Code is logged in as.
pub fn live_email(root: &Path) -> String {
    let config: Value =
        serde_json::from_slice(&fs::read(root.join("home/.claude.json")).unwrap()).unwrap();
    config["oauthAccount"]["emailAddress"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A running binary whose stdout and stderr are read on threads of their own while it runs, so
/// one that prints without end (an `auto` loop) never blocks on a full pipe, and what it has
/// printed so far can be read at any time.
pub struct Running {
    pub child: Child,
    out: Arc<Mutex<Vec<u8>>>,
    err: Arc<Mutex<Vec<u8>>>,
    readers: Vec<JoinHandle<()>>,
}

impl Running {
    /// Spawns `cmd` with its stdout and stderr piped, each drained on a thread of its own.
    pub fn spawn(cmd: std::process::Command) -> Self {
        Self::spawn_reading(cmd, None)
    }

    /// As `spawn`, but its stdout is read only until `lines` lines are in, then closed with the
    /// reader gone: the next write to it fails, as for `tagteam auto --json | head -n 1`.
    pub fn spawn_closing_stdout_after(cmd: std::process::Command, lines: usize) -> Self {
        Self::spawn_reading(cmd, Some(lines))
    }

    fn spawn_reading(mut cmd: std::process::Command, stdout_lines: Option<usize>) -> Self {
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (out, err) = (Arc::default(), Arc::default());
        let readers = vec![
            drain(child.stdout.take().unwrap(), Arc::clone(&out), stdout_lines),
            drain(child.stderr.take().unwrap(), Arc::clone(&err), None),
        ];
        Running {
            child,
            out,
            err,
            readers,
        }
    }

    /// Everything it has printed on stdout so far.
    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.lock().unwrap()).into_owned()
    }

    /// Polls every 10 ms until `ready` holds for its stdout so far. Fails the test if it exits
    /// first, or if `within` passes (killing it, so a hung one is not left behind).
    pub fn wait_for(&mut self, within: Duration, what: &str, ready: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + within;
        while !ready(&self.stdout()) {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("tagteam exited ({status}) before {what}");
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!("tagteam never got to {what}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Sends `signal`, as a terminal's Ctrl-C (SIGINT) or `kill` would.
    pub fn signal(&self, signal: i32) {
        // SAFETY: kill(2) reads no memory of ours. The child is one this test spawned and has
        // not reaped, so its pid names it and no other process.
        let rc = unsafe { libc::kill(self.child.id() as libc::pid_t, signal) };
        assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());
    }

    /// Its status and everything it printed, once it has exited. Fails the test, killing it,
    /// if it is still running after `within`.
    pub fn finish(mut self, within: Duration) -> Output {
        let deadline = Instant::now() + within;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                panic!("tagteam was still running {within:?} later");
            }
            thread::sleep(Duration::from_millis(10));
        };
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        Output {
            status,
            stdout: std::mem::take(&mut *self.out.lock().unwrap()),
            stderr: std::mem::take(&mut *self.err.lock().unwrap()),
        }
    }
}

/// Reads `from` into `into` until its end, or, with `lines`, until that many lines are in
/// (the pipe is dropped then, and the writer's next write fails), on a thread of its own.
fn drain(
    mut from: impl Read + Send + 'static,
    into: Arc<Mutex<Vec<u8>>>,
    lines: Option<usize>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 {
                break;
            }
            let mut into = into.lock().unwrap();
            into.extend_from_slice(&buf[..n]);
            if lines.is_some_and(|l| into.iter().filter(|b| **b == b'\n').count() >= l) {
                break;
            }
        }
    })
}

/// `id`'s session profile under `root`, as `tagteam run` leaves it for Claude Code (§12.2): the
/// directory and its marker, whose outer home is the default one. Returns the profile and its
/// exported spelling, the `CLAUDE_CONFIG_DIR` its run shell sees.
pub fn cc_profile(root: &Path, id: &str) -> (PathBuf, String) {
    let env = Env::for_test(root);
    let id = AccountId::from_string(id);
    let profile = profile_path(&env, &id);
    ensure_private_dir(&profile).unwrap();
    let cc = ClaudeCode::new(Arc::new(FakeKeychain::new()), Platform::MacOs);
    let spelling = cc.profile_spelling(&canonical_profile_path(&profile).unwrap());
    ProfileMarker {
        provider: ProviderId::new(CLAUDE_CODE),
        account_id: id,
        config_dir: spelling.clone(),
        outer: cc.outer_home(&env),
    }
    .write(&profile)
    .unwrap();
    (profile, spelling)
}

/// A live launch reservation of `profile` (§12.5) while the guard lives: a locked
/// `.tagteam-launch/4242.lock`, which the binary's non-blocking probe sees as held.
pub fn hold_launch(profile: &Path) -> FlockGuard {
    FlockGuard::try_lock(&profile.join(LAUNCH_DIR).join("4242.lock"))
        .unwrap()
        .expect("nothing else holds the reservation")
}

/// Decision 11's fake `claude`: a `/bin/sh` script, so no test ever runs the real one. See
/// `fake_claude` for the variables that steer it.
const FAKE_CLAUDE: &str = r##"#!/bin/sh
# tagteam's fake `claude` for the CLI tests (tests/common/mod.rs, `fake_claude`). Every
# FAKE_CLAUDE_* variable is optional; nothing here reaches the network or the real HOME.
me=$$
home=${CLAUDE_CONFIG_DIR:-$HOME/.claude}
if [ -n "$CLAUDE_CONFIG_DIR" ]; then config=$CLAUDE_CONFIG_DIR/.claude.json; else config=$HOME/.claude.json; fi
record=
sleeper=

note() {
  if [ -n "$FAKE_CLAUDE_OUT" ]; then
    printf '%s %s %s\n' "$me" "$1" "$2" >> "$FAKE_CLAUDE_OUT"
  fi
}

finish() {
  if [ -n "$sleeper" ]; then kill "$sleeper" 2>/dev/null; fi
  if [ -n "$record" ]; then rm -f "$record"; fi
  note exit "$1"
  exit "$1"
}

caught() {
  note signal "$1"
  if [ "$FAKE_CLAUDE_ON_SIGNAL" != continue ]; then finish $((128 + $1)); fi
}
trap 'caught 1' HUP
trap 'caught 2' INT
trap 'caught 15' TERM

pause() {
  sleep "$1" &
  sleeper=$!
  while :; do
    wait "$sleeper"
    if [ "$?" -le 128 ] || ! kill -0 "$sleeper" 2>/dev/null; then break; fi
  done
  sleeper=
}

# A string field of the config's `oauthAccount`, as Claude Code pretty-prints it.
field() {
  sed -n "s/.*\"$1\": *\"\([^\"]*\)\".*/\1/p" "$config" 2>/dev/null | head -n 1
}

mode=session
if [ "$1" = auth ] && [ "$2" = status ]; then mode=auth; fi
note call "$mode"
note ppid "$PPID"
note cwd "$(pwd -P)"
for a in "$@"; do note arg "$a"; done
if [ -n "$FAKE_CLAUDE_OUT" ]; then
  /usr/bin/env | while IFS= read -r line; do note env "$line"; done
fi

if [ "$mode" = auth ]; then
  if [ -n "$FAKE_CLAUDE_AUTH_SLEEP" ]; then pause "$FAKE_CLAUDE_AUTH_SLEEP"; fi
  if [ -n "$FAKE_CLAUDE_AUTH" ]; then
    printf '%s\n' "$FAKE_CLAUDE_AUTH"
    finish "${FAKE_CLAUDE_AUTH_EXIT:-0}"
  fi
  if grep -q '"apiKeyHelper"' "$home/settings.json" 2>/dev/null; then
    printf '{"loggedIn":true,"authMethod":"api_key_helper","apiProvider":"firstParty","apiKeySource":"apiKeyHelper","configDirectory":"%s"}\n' "$home"
    finish "${FAKE_CLAUDE_AUTH_EXIT:-0}"
  fi
  email=$FAKE_CLAUDE_EMAIL
  org=$FAKE_CLAUDE_ORG
  if [ -z "$email" ] && [ -f "$home/.credentials.json" ]; then
    email=$(field emailAddress)
    org=$(field organizationUuid)
  fi
  if [ -n "$email" ]; then
    if [ -n "$org" ]; then org=",\"orgId\":\"$org\""; fi
    printf '{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","configDirectory":"%s","email":"%s"%s,"subscriptionType":"max"}\n' \
      "$home" "$email" "$org"
    finish "${FAKE_CLAUDE_AUTH_EXIT:-0}"
  fi
  printf '{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty","configDirectory":"%s"}\n' "$home"
  finish "${FAKE_CLAUDE_AUTH_EXIT:-1}"
fi

if [ -n "$FAKE_CLAUDE_RECORD" ]; then
  mkdir -p "$home/sessions" && chmod 700 "$home/sessions"
  started=$(LC_ALL=C TZ=UTC ps -o lstart= -p "$me" 2>/dev/null | sed 's/^ *//;s/ *$//')
  proc=
  if [ -n "$started" ]; then proc=",\"procStart\":\"$started\""; fi
  record="$home/sessions/$me.json"
  printf '{"pid":%s,"sessionId":"fake-%s","cwd":"%s","startedAt":%s000,"kind":"%s"%s}\n' \
    "$me" "$me" "$(pwd -P)" "$(date +%s)" "$FAKE_CLAUDE_RECORD" "$proc" > "$record"
fi
if [ -n "$FAKE_CLAUDE_ROTATE" ]; then
  (umask 077 && printf '%s' "$FAKE_CLAUDE_ROTATE" > "$home/.credentials.json.fake" \
    && mv -f "$home/.credentials.json.fake" "$home/.credentials.json")
fi
note ready
if [ -n "$FAKE_CLAUDE_HOLD" ]; then
  n=0
  while [ ! -e "$FAKE_CLAUDE_HOLD" ] && [ "$n" -lt 600 ]; do
    pause 0.05
    n=$((n + 1))
  done
elif [ -n "$FAKE_CLAUDE_SLEEP" ]; then
  pause "$FAKE_CLAUDE_SLEEP"
fi
finish "${FAKE_CLAUDE_EXIT:-0}"
"##;

/// Writes Decision 11's fake `claude` to `<root>/bin/claude` (0755) and returns `<root>/bin`,
/// to put first on `PATH` (`path_with`). Its config home is `CLAUDE_CONFIG_DIR`, else
/// `$HOME/.claude`. This is the one list of the variables that steer it, each optional; Tasks
/// 12 and 13 use no others:
/// - `FAKE_CLAUDE_OUT`: a file every run appends its record to (`fake_claude_calls`): its pid,
///   parent, directory, arguments and environment, then `ready` once a session runs, the
///   signals it caught, and its exit.
/// - `FAKE_CLAUDE_EXIT`: the session's exit code (default 0).
/// - `FAKE_CLAUDE_SLEEP`: seconds the session runs before it exits, when `FAKE_CLAUDE_HOLD` is
///   unset.
/// - `FAKE_CLAUDE_HOLD`: a path; the session runs until it exists, for at most 30 s.
/// - `FAKE_CLAUDE_RECORD`: a session-record kind (`interactive`, `bg`, `daemon`). The session
///   writes its record `<home>/sessions/<pid>.json` with it (Appendix A.7: `pid`, `startedAt`,
///   `kind`, and `procStart` when `ps` answers), and a graceful exit removes it. SIGKILL leaves
///   it behind, as for Claude Code.
/// - `FAKE_CLAUDE_ROTATE`: credential JSON the session writes to `<home>/.credentials.json`,
///   0600, by rename, at its start: a rotation of the profile's credential.
/// - `FAKE_CLAUDE_ON_SIGNAL`: `continue` records SIGHUP, SIGINT and SIGTERM and keeps running;
///   otherwise each is recorded and ends the session gracefully with 128 + its number. SIGQUIT
///   keeps its default action.
/// - `FAKE_CLAUDE_AUTH`: `claude auth status`'s reply, verbatim, exiting with
///   `FAKE_CLAUDE_AUTH_EXIT` (default 0).
/// - Without `FAKE_CLAUDE_AUTH`, `auth status` answers from its config home, as Claude Code's
///   does, with `configDirectory` the config home it was given:
///   - `api_key_helper` when `<home>/settings.json` names an `apiKeyHelper`;
///   - else a `claude.ai` login as `FAKE_CLAUDE_EMAIL` (and `FAKE_CLAUDE_ORG`'s `orgId`), or,
///     without them, as the config's `oauthAccount` email and org when `<home>/.credentials.json`
///     exists;
///   - else logged out, exiting 1.
/// - `FAKE_CLAUDE_AUTH_SLEEP`: seconds `auth status` takes before it answers.
///
/// Paths and values go into JSON unescaped, so tests keep them free of quotes and
/// backslashes, and records are lines, so no argument or variable may hold a newline.
pub fn fake_claude(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let path = bin.join("claude");
    fs::write(&path, FAKE_CLAUDE).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// `bin`, then this test process's own `PATH`: what puts the fake `claude` first.
pub fn path_with(bin: &Path) -> OsString {
    let mut path = bin.as_os_str().to_owned();
    if let Some(rest) = std::env::var_os("PATH") {
        path.push(":");
        path.push(rest);
    }
    path
}

/// One run of the fake `claude`, as it recorded itself in `FAKE_CLAUDE_OUT`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeCall {
    pub pid: u32,
    /// `auth` for `claude auth status`, else `session`.
    pub mode: String,
    pub ppid: u32,
    /// Its working directory, symlinks resolved.
    pub cwd: PathBuf,
    /// Its arguments, as the bytes it was given.
    pub args: Vec<OsString>,
    pub env: BTreeMap<String, String>,
    /// The session got as far as running: its record and rotation are written.
    pub ready: bool,
    /// The SIGHUP, SIGINT and SIGTERM it caught, by number, in order.
    pub signals: Vec<i32>,
    pub exit: Option<i32>,
}

/// Every run recorded in `out`, in the order they started; a missing file is no runs. Each
/// line is `<pid> <key> <value>`, so runs that overlap never mix. Lines are read as bytes: an
/// argument keeps its own, and every other value is read as UTF-8, lossily.
pub fn fake_claude_calls(out: &Path) -> Vec<FakeCall> {
    let bytes = fs::read(out).unwrap_or_default();
    let mut calls: Vec<FakeCall> = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        let mut parts = line.splitn(3, |b| *b == b' ');
        let (Some(pid), Some(key)) = (parts.next(), parts.next()) else {
            continue;
        };
        let Ok(pid) = String::from_utf8_lossy(pid).parse::<u32>() else {
            continue;
        };
        let raw = parts.next().unwrap_or(b"");
        let value = String::from_utf8_lossy(raw);
        let key = String::from_utf8_lossy(key);
        if key == "call" {
            calls.push(FakeCall {
                pid,
                mode: value.into_owned(),
                ..FakeCall::default()
            });
            continue;
        }
        let Some(call) = calls.iter_mut().rev().find(|c| c.pid == pid) else {
            continue;
        };
        match key.as_ref() {
            "ppid" => call.ppid = value.parse().unwrap_or(0),
            "cwd" => call.cwd = PathBuf::from(OsString::from_vec(raw.to_vec())),
            "arg" => call.args.push(OsString::from_vec(raw.to_vec())),
            "env" => {
                if let Some((k, v)) = value.split_once('=') {
                    call.env.insert(k.to_owned(), v.to_owned());
                }
            }
            "ready" => call.ready = true,
            "signal" => call.signals.extend(value.parse::<i32>().ok()),
            "exit" => call.exit = value.parse().ok(),
            _ => {}
        }
    }
    calls
}
