//! The profile phase: the scratch default home holds no login, and the test account runs in its
//! session profile through `tagteam run`, which captures whatever CC rotates there (§12.5).

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_cc::ItemKind;
use tagteam_cc::locks::{STORAGE_WRITE_STALE, acquire_storage_write};
use tagteam_provider::atomic::write_atomic_private;
use tagteam_provider::{
    Cancel, MkdirLock, MkdirLockSpec, Read, SystemProcessProbe, read_supervisor_lock,
    record_is_live,
};

use super::{changes, new_record, read_json, records, snapshot};
use crate::compat::ctx::{ALIAS_OAUTH, ALIAS_SETUP_TOKEN, Ctx, MODEL, PROMPT, generation, now_ms};
use crate::compat::daemon;
use crate::compat::report::{Outcome, Probe};
use crate::compat::sys::{HarnessError, cancel, harness, pause, signal, wait_until};

const STATUS: [&str; 3] = ["auth", "status", "--json"];
const REQUEST: [&str; 6] = ["-p", PROMPT, "--model", MODEL, "--max-turns", "1"];
/// An identity that is not the account's, for §12.3's `invalid` outcome.
const OTHER_EMAIL: &str = "tagteam-compat-other@example.invalid";
/// The entries §12.2 shares that are files, not directories.
const SHARED_FILES: [&str; 4] = [
    "settings.json",
    "history.jsonl",
    "CLAUDE.md",
    "keybindings.json",
];
/// How long the harness holds a lock that CC must wait for.
const HOLD: Duration = Duration::from_secs(3);

pub fn read_name<T>(r: &Read<T>) -> &'static str {
    match r {
        Read::Present(_) => "present",
        Read::Absent => "absent",
        Read::Unreadable(_) => "unreadable",
    }
}

fn mode(path: &Path) -> Option<u32> {
    fs::symlink_metadata(path)
        .ok()
        .map(|m| m.permissions().mode() & 0o777)
}

/// A login in `home` that can never refresh: the account's credential with a refresh token
/// that is not one, its access token marked valid for a day, and `oauth_account` as identity.
fn seed_inert_login(
    home: &str,
    credential: &Value,
    oauth_account: &Value,
) -> Result<(), HarnessError> {
    let home = Path::new(home);
    let mut oauth = credential["claudeAiOauth"].clone();
    oauth["refreshToken"] = json!("sk-ant-ort01-tagteam-compat-inert");
    oauth["expiresAt"] = json!(now_ms() + 86_400_000);
    let cred = json!({"claudeAiOauth": oauth});
    write_atomic_private(
        &home.join(".credentials.json"),
        &serde_json::to_vec(&cred).expect("a Value serializes"),
        0o600,
    )?;
    let config = json!({"oauthAccount": oauth_account, "hasCompletedOnboarding": true});
    write_atomic_private(
        &home.join(".claude.json"),
        &serde_json::to_vec_pretty(&config).expect("a Value serializes"),
        0o600,
    )?;
    Ok(())
}

/// §12.3 step 8's table, from CC's side: what `claude auth status --json` reports in each case
/// tagteam tells apart, `configDirectory` as the exported spelling, and a setup token's method.
pub fn auth_status(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();

    // valid: tagteam's own launch bootstraps the profile and validates it with this very
    // command (§12.3 step 8) before it runs it.
    let ran = ctx.run_profile(&STATUS)?;
    p.note("valid: tagteam run", ran.summary());
    let (dir, spelling) = ctx
        .profile()?
        .ok_or_else(|| harness("tagteam run made no profile"))?;
    let v = ran.json().unwrap_or(Value::Null);
    p.expect_eq("valid: exit", 0, json!(ran.code));
    p.expect_eq("valid: loggedIn", true, v["loggedIn"].clone());
    p.expect_eq("valid: authMethod", "claude.ai", v["authMethod"].clone());
    p.expect_eq(
        "valid: configDirectory is the exported spelling",
        spelling.as_str(),
        v["configDirectory"].clone(),
    );
    p.expect(
        "valid: email is the account's",
        v["email"].as_str() == Some(ctx.oauth.email.as_str()),
        json!(v["email"].is_string()),
    );

    // invalid, logged out.
    let empty = ctx.new_home("logged-out")?;
    let r = ctx.claude(&empty, &STATUS).run(&ctx.roots)?;
    let v = r.json().unwrap_or(Value::Null);
    p.expect("logged out: exits non-zero", !r.success(), json!(r.code));
    p.expect_eq("logged out: loggedIn", false, v["loggedIn"].clone());
    p.expect_eq("logged out: authMethod", "none", v["authMethod"].clone());
    p.expect_eq(
        "logged out: configDirectory",
        empty.as_str(),
        v["configDirectory"].clone(),
    );

    let credential: Value = serde_json::from_slice(&ctx.vault_credential(&ctx.oauth.id)?)
        .map_err(|_| harness("the vault credential is not JSON"))?;
    let account = read_json(&dir.join(".claude.json"))
        .map(|c| c["oauthAccount"].clone())
        .filter(Value::is_object)
        .ok_or_else(|| harness("the profile's .claude.json has no oauthAccount"))?;

    // invalid, another identity: CC reports the config's email, which tagteam compares.
    let other = ctx.new_home("other-identity")?;
    let mut someone = account.clone();
    someone["emailAddress"] = json!(OTHER_EMAIL);
    seed_inert_login(&other, &credential, &someone)?;
    let v = ctx
        .claude(&other, &STATUS)
        .run(&ctx.roots)?
        .json()
        .unwrap_or(Value::Null);
    p.expect_eq("another identity: loggedIn", true, v["loggedIn"].clone());
    p.expect_eq(
        "another identity: authMethod",
        "claude.ai",
        v["authMethod"].clone(),
    );
    p.expect_eq("another identity: email", OTHER_EMAIL, v["email"].clone());

    // overridden: an apiKeyHelper in the settings a profile shares.
    let helper = ctx.new_home("api-key-helper")?;
    seed_inert_login(&helper, &credential, &account)?;
    fs::write(
        Path::new(&helper).join("settings.json"),
        json!({"apiKeyHelper": "echo sk-ant-api03-tagteam-compat-helper"}).to_string(),
    )?;
    let v = ctx
        .claude(&helper, &STATUS)
        .run(&ctx.roots)?
        .json()
        .unwrap_or(Value::Null);
    p.expect_eq(
        "overridden by apiKeyHelper: authMethod",
        "api_key_helper",
        v["authMethod"].clone(),
    );
    p.note(
        "overridden by apiKeyHelper: apiKeySource",
        ctx.redact.value(&v["apiKeySource"]),
    );

    // overridden: a token in the environment, which run scrubs (§12.5).
    let v = ctx
        .claude(&other, &STATUS)
        .var(
            "CLAUDE_CODE_OAUTH_TOKEN",
            "sk-ant-oat01-tagteam-compat-dummy",
        )
        .run(&ctx.roots)?
        .json()
        .unwrap_or(Value::Null);
    p.expect_eq(
        "overridden by CLAUDE_CODE_OAUTH_TOKEN: authMethod",
        "oauth_token",
        v["authMethod"].clone(),
    );

    // drifted is never CC's doing when it reports the spelling it was given, a link's
    // included, and never the link's target (Appendix A.1).
    let link = ctx.layout.homes().join("linked");
    std::os::unix::fs::symlink(&other, &link)?;
    let linked = link.to_string_lossy().into_owned();
    ctx.roots.services(&linked)?;
    let v = ctx
        .claude(&linked, &STATUS)
        .run(&ctx.roots)?
        .json()
        .unwrap_or(Value::Null);
    p.expect_eq(
        "a linked home: configDirectory is the link's spelling",
        linked.as_str(),
        v["configDirectory"].clone(),
    );

    // A setup token's account (§12.3 infers `claude.ai`).
    match &ctx.setup_token {
        None => {
            p.expect(
                "setup token: the compat store holds one",
                false,
                json!("run `cargo xtask compat login` again to add it"),
            );
        }
        Some(st) => {
            let ran = ctx.run_as(ALIAS_SETUP_TOKEN, &STATUS).run(&ctx.roots)?;
            p.note("setup token: tagteam run", ran.summary());
            let v = match ran.json() {
                Some(v) => v,
                // tagteam's validation refused the launch: ask CC in that profile directly.
                None => match ctx.profile_of(&st.id)? {
                    Some((_, sp)) => ctx
                        .claude(&sp, &STATUS)
                        .run(&ctx.roots)?
                        .json()
                        .unwrap_or(Value::Null),
                    None => Value::Null,
                },
            };
            p.expect_eq("setup token: loggedIn", true, v["loggedIn"].clone());
            p.expect_eq(
                "setup token: authMethod",
                "claude.ai",
                v["authMethod"].clone(),
            );
        }
    }
    Ok(p.finish("CC answers every case as §12.3's table reads it"))
}

/// Every Keychain item of `spelling`: present or not, and when it was last written.
fn item_states(ctx: &Ctx, spelling: &str) -> Result<Value, HarnessError> {
    if !ctx.macos {
        return Ok(Value::Null);
    }
    let mut out = serde_json::Map::new();
    for kind in [ItemKind::OAuth, ItemKind::ManagedKey] {
        let item = ctx.item(spelling, kind)?;
        out.insert(
            item.service.clone(),
            json!({"read": read_name(&item.exists()), "modified": item.modified()}),
        );
    }
    Ok(Value::Object(out))
}

/// §13.6 relies on both halves of Appendix A.7: `claude auth status` writes nothing in a home
/// that has its global config (the profile's, its Keychain items included), and in one that has
/// none CC's start-up creates its config first (`.claude.json`, a `.claude.json.lock` it leaves,
/// `backups/`). The second half is evidence, never a failure; run again in the home it made,
/// `auth status` writes nothing. Doctor runs `claude --version` before its gate on the config's
/// existence, so what `--version` does to a never-started home is recorded too, as information
/// only.
pub fn auth_status_read_only(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let (_, profile) = ctx.profile_ready()?;

    // A home that has its global config: nothing is created, changed or removed.
    let before = snapshot(Path::new(&profile))?;
    let items = item_states(ctx, &profile)?;
    let r = ctx.claude(&profile, &STATUS).run(&ctx.roots)?;
    p.note("the profile: claude auth status", r.summary());
    let found = changes(&before, &snapshot(Path::new(&profile))?);
    p.expect(
        "the profile: nothing created, changed or removed",
        found.is_empty(),
        json!(found),
    );
    p.expect_eq(
        "the profile: its Keychain items as they were",
        items,
        item_states(ctx, &profile)?,
    );

    // An empty home, `claude --version` first, in a home of its own and otherwise the same.
    let version_home = ctx.new_home("read-only-version")?;
    let before = snapshot(Path::new(&version_home))?;
    let r = ctx.claude(&version_home, &["--version"]).run(&ctx.roots)?;
    p.note("an empty home: claude --version", r.summary());
    p.note(
        "an empty home: what claude --version created, changed or removed (information only)",
        json!(changes(&before, &snapshot(Path::new(&version_home))?)),
    );

    // An empty home: what CC creates is evidence, expected, not a failure.
    let empty = ctx.new_home("read-only")?;
    let before = snapshot(Path::new(&empty))?;
    let items = item_states(ctx, &empty)?;
    let r = ctx.claude(&empty, &STATUS).run(&ctx.roots)?;
    p.note("an empty home: claude auth status", r.summary());
    let created = changes(&before, &snapshot(Path::new(&empty))?);
    p.note(
        "an empty home: what claude auth status created, changed or removed (expected)",
        json!(created),
    );
    p.expect_eq(
        "an empty home: its Keychain items as they were",
        items,
        item_states(ctx, &empty)?,
    );

    // The home it made now has its global config: a second run writes nothing.
    let before = snapshot(Path::new(&empty))?;
    let r = ctx.claude(&empty, &STATUS).run(&ctx.roots)?;
    p.note("the same home again: claude auth status", r.summary());
    let found = changes(&before, &snapshot(Path::new(&empty))?);
    p.expect(
        "the same home again, now with a global config: nothing created, changed or removed",
        found.is_empty(),
        json!(found),
    );
    Ok(p.finish(
        "claude auth status wrote nothing in a home with its global config; what it creates in an empty one is recorded",
    ))
}

/// Appendix A.2 and A.3: a freshly bootstrapped profile holds its credential in the file and no
/// item; CC's first credential write lands in the item named from the exported spelling, and
/// the file goes. tagteam captures the rotation when the session ends.
pub fn profile_keychain_item(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let (dir, spelling) = ctx.fresh_profile()?;
    let canonical = fs::canonicalize(&dir)?.to_string_lossy().into_owned();
    p.expect_eq(
        "the exported spelling is the profile's canonical path (§12.2)",
        canonical,
        spelling.as_str(),
    );
    let item = ctx.item(&spelling, ItemKind::OAuth)?;
    p.note("the item named from that spelling", json!(item.service));
    let file = dir.join(".credentials.json");
    let exists = item.exists();
    p.expect(
        "after the bootstrap: no item (§12.3 step 5)",
        matches!(exists, Read::Absent),
        json!(read_name(&exists)),
    );
    p.expect_eq(
        "after the bootstrap: .credentials.json, 0600",
        json!("600"),
        json!(mode(&file).map(|m| format!("{m:o}"))),
    );
    let before = fs::read(&file).ok().and_then(|b| generation(&b));
    p.note("expired", ctx.expire(&spelling)?);
    let ran = ctx.run_profile(&REQUEST)?;
    p.expect("claude -p succeeded", ran.success(), ran.summary());
    let after = match item.read() {
        Read::Present(b) => generation(&b),
        _ => None,
    };
    p.expect(
        "after CC's write: the item holds the credential",
        after.is_some(),
        json!(after),
    );
    p.expect(
        "after CC's write: .credentials.json is gone (Appendix A.3)",
        !file.exists(),
        json!(file.exists()),
    );
    p.expect(
        "CC refreshed: a new generation",
        after.is_some() && after != before,
        json!({"before": before, "after": after}),
    );
    let vault = generation(&ctx.vault_credential(&ctx.oauth.id)?);
    p.expect_eq(
        "tagteam captured it when the session ended",
        json!(after),
        json!(vault),
    );
    Ok(p.finish("CC moved the bootstrapped file into the item named from the spelling"))
}

/// Appendix A.4: CC writes `expiresAt` as an integer of epoch milliseconds.
pub fn expires_at_integer(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let (_, spelling) = ctx.profile_ready()?;
    p.note("expired", ctx.expire(&spelling)?);
    let ran = ctx.run_profile(&REQUEST)?;
    p.expect("claude -p succeeded", ran.success(), ran.summary());
    let (bytes, place) = ctx
        .read_credential(&spelling)?
        .ok_or_else(|| harness("the profile holds no credential after the refresh"))?;
    p.note("CC wrote it to", place.describe());
    let v: Value =
        serde_json::from_slice(&bytes).map_err(|_| harness("the credential is not JSON"))?;
    let raw = &v["claudeAiOauth"]["expiresAt"];
    let text = raw.to_string();
    p.expect(
        "expiresAt is a JSON integer",
        raw.is_number() && text.bytes().all(|b| b.is_ascii_digit()),
        json!(text),
    );
    p.expect(
        "expiresAt is ahead: CC refreshed and wrote it",
        raw.as_i64().is_some_and(|t| t > now_ms()),
        json!(raw.as_i64().map(|t| t - now_ms())),
    );
    Ok(p.finish("CC wrote expiresAt as an integer"))
}

fn lines(path: &Path) -> usize {
    fs::read_to_string(path).map_or(0, |s| s.lines().count())
}

/// §12.2: `settings.json` is written through one link (`claude auto-mode reset` rewrites the
/// user settings), `history.jsonl` is appended through one (an interactive prompt), and
/// `CLAUDE.md` and `keybindings.json` are reported as CC leaves them across those sessions.
pub fn shared_writes(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let live = ctx.layout.live();
    let (dir, _) = ctx.profile_ready()?;
    let kept: Vec<Option<Vec<u8>>> = SHARED_FILES
        .iter()
        .map(|n| fs::read(live.join(n)).ok())
        .collect();
    for name in SHARED_FILES {
        let target = fs::read_link(dir.join(name)).ok();
        p.expect(
            &format!("{name}: a link to the default home's file"),
            target.as_deref() == Some(live.join(name).as_path()),
            json!(target),
        );
    }

    // settings.json: rewritten through the link.
    fs::write(
        live.join("settings.json"),
        json!({"autoMode": {}, "tagteamCompat": "kept"}).to_string(),
    )?;
    let ran = ctx.run_profile(&["auto-mode", "reset", "--yes"])?;
    p.note("claude auto-mode reset", ran.summary());
    let settings = read_json(&live.join("settings.json")).unwrap_or(Value::Null);
    p.expect(
        "settings.json: CC's write reached the default home's file",
        settings.get("autoMode").is_none() && settings["tagteamCompat"] == "kept",
        settings.clone(),
    );
    p.expect(
        "settings.json: still a link",
        fs::read_link(dir.join("settings.json")).is_ok(),
        json!(mode(&dir.join("settings.json")).map(|m| format!("{m:o}"))),
    );

    // history.jsonl: appended through the link by an interactive prompt.
    let history = live.join("history.jsonl");
    let before_lines = lines(&history);
    let sessions = dir.join("sessions");
    let before = records(&sessions);
    let mut pty = ctx.profile_pty(&["--model", MODEL])?;
    let started = new_record(&sessions, &before, Duration::from_secs(60)).is_some();
    let mut appended = false;
    if started {
        pause(Duration::from_secs(2));
        pty.line(PROMPT)?;
        appended = wait_until(Duration::from_secs(30), || lines(&history) > before_lines);
        pause(Duration::from_secs(15));
    }
    p.expect(
        "history.jsonl: the session started",
        started,
        json!(pty.transcript(600)),
    );
    let _ = pty.line("/exit");
    let code = pty.finish(Duration::from_secs(30))?;
    p.note("the session's exit", json!(code));
    let last = fs::read_to_string(&history)
        .ok()
        .and_then(|s| s.lines().last().map(str::to_owned))
        .unwrap_or_default();
    p.expect(
        "history.jsonl: the prompt was appended to the default home's file",
        appended && last.contains(PROMPT),
        json!({"linesBefore": before_lines, "linesAfter": lines(&history)}),
    );
    p.expect(
        "history.jsonl: still a link",
        fs::read_link(dir.join("history.jsonl")).is_ok(),
        json!(null),
    );

    // CLAUDE.md and keybindings.json: what CC did to them over these sessions.
    for (i, name) in SHARED_FILES.iter().enumerate().skip(2) {
        let link = fs::read_link(dir.join(name)).ok();
        let now = fs::read(live.join(name)).ok();
        p.expect(
            &format!("{name}: still a link"),
            link.is_some(),
            json!({"contentUnchanged": now == kept[i]}),
        );
    }
    Ok(p.finish("CC wrote and appended through the links and left the others linked"))
}

/// Appendix A.7: a record when a session starts, removed by a graceful exit on SIGINT, SIGTERM
/// and SIGHUP, its `procStart` what `ps -o lstart=` reports; `claude --bg` runs a supervisor
/// that is `daemon.lock` (a live pid) with its workers' records in the profile, all of it gone
/// after `claude daemon stop --any`.
pub fn session_records(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let (_, spelling) = ctx.profile_ready()?;
    let sessions = Path::new(&spelling).join("sessions");
    for sig in ["INT", "TERM", "HUP"] {
        let before = records(&sessions);
        let pty = ctx.profile_pty(&["--model", MODEL])?;
        let Some((path, record)) = new_record(&sessions, &before, Duration::from_secs(60)) else {
            p.expect(
                &format!("SIG{sig}: a record at the start"),
                false,
                json!(pty.transcript(600)),
            );
            pty.finish(Duration::from_secs(5))?;
            continue;
        };
        p.expect(
            &format!("SIG{sig}: a record at the start"),
            true,
            json!({"kind": record.kind, "pid": record.pid}),
        );
        let ps = super::lstart(record.pid);
        let parsed = record
            .proc_start
            .as_deref()
            .and_then(tagteam_provider::parse_lstart);
        p.expect(
            &format!("SIG{sig}: procStart is ps's lstart"),
            parsed.is_some() && record.proc_start.as_deref().map(str::trim) == ps.as_deref(),
            json!({"procStart": record.proc_start, "ps": ps}),
        );
        signal(record.pid, sig, false);
        let gone = wait_until(Duration::from_secs(20), || !path.exists());
        p.expect(
            &format!("SIG{sig}: the record is removed"),
            gone,
            json!(null),
        );
        p.note(
            &format!("SIG{sig}: tagteam's exit"),
            json!(pty.finish(Duration::from_secs(30))?),
        );
    }

    // A background session's supervisor (`claude --bg`), in the profile. It leaves the groups
    // the harness ends, so cleanup must stop it if this check cannot. In 2.1.292 the supervisor
    // writes no session record: it is `daemon.lock` (a live pid), and its workers write
    // records (Appendix A.7).
    ctx.must_stop(&spelling);
    let ran = ctx.run_profile(&["--bg", "--model", MODEL, PROMPT])?;
    p.note("claude --bg", ran.summary());
    let home = Path::new(&spelling);
    let (mut supervisor, mut workers, mut kinds) = (false, false, Vec::new());
    wait_until(Duration::from_secs(60), || {
        supervisor = matches!(
            read_supervisor_lock(&home.join("daemon.lock")),
            Read::Present(r) if record_is_live(&SystemProcessProbe, &r, "claude")
        );
        let live: Vec<_> = records(&sessions)
            .into_iter()
            .filter(|(_, r)| record_is_live(&SystemProcessProbe, r, "claude"))
            .collect();
        kinds = live.iter().filter_map(|(_, r)| r.kind.clone()).collect();
        workers = live
            .iter()
            .any(|(_, r)| matches!(r.kind.as_deref(), Some("bg" | "daemon-worker")));
        supervisor && workers
    });
    p.expect(
        "--bg: daemon.lock names a live supervisor",
        supervisor,
        json!(null),
    );
    p.expect("--bg: a worker's session record", workers, json!(kinds));
    p.note("--bg: the kinds of the live records", json!(kinds));
    let shape = daemon::lock_shape(home);
    p.expect(
        "--bg: daemon.lock's procStart is ps's lstart text (judged like a record's, Appendix A.7)",
        shape["procStart"] == "lstart text",
        shape.clone(),
    );

    // While it runs, the profile is session-owned for tagteam: doctor reports the daemon, and a
    // switch to the account is refused as `session-owned` (§12.6, §13.6). Only when both the
    // supervisor and a worker were seen: otherwise the profile is not known to be owned.
    if supervisor && workers {
        let doctor = ctx
            .tagteam(&["doctor", "--json"])
            .timeout(Duration::from_secs(60))
            .run(&ctx.roots)?;
        p.note("tagteam doctor --json", doctor.summary());
        let report = doctor.json().unwrap_or(Value::Null);
        p.expect(
            "--bg: doctor's `sessions.daemon` info line names this profile",
            doctor_names_daemon(&report, &spelling),
            ctx.redact.value(&daemon_entries(&report)),
        );
        let switch = ctx
            .tagteam(&["switch", ALIAS_OAUTH, "--json"])
            .run(&ctx.roots)?;
        p.note("tagteam switch while it runs", switch.summary());
        p.expect(
            "--bg: a switch to the account is refused as session-owned",
            !switch.success()
                && switch.json().as_ref().and_then(error_kind) == Some("session-owned"),
            json!(null),
        );
    } else {
        p.note(
            "--bg: doctor and the switch while it runs",
            json!("not run: the supervisor and a worker were not both seen"),
        );
    }

    // A failed stop is a failed expectation with its error, not a lost check: the evidence so
    // far stays, and cleanup (`must_stop`) and the later quiescence assertion stand.
    stop_outcome(
        &mut p,
        ctx.stop_daemon(&spelling),
        cancel().requested().is_some(),
    )?;
    let left = daemon::survey(home, &SystemProcessProbe);
    p.expect(
        "--bg: the lock, the roster and the records name nothing alive once it stops",
        left.is_clear(),
        json!(left.describe()),
    );
    p.expect(
        "--bg: daemon.lock is gone or names a dead process",
        !matches!(
            read_supervisor_lock(&home.join("daemon.lock")),
            Read::Present(r) if record_is_live(&SystemProcessProbe, &r, "claude")
        ),
        json!(null),
    );
    Ok(p.finish("records come and go as Appendix A.7 says"))
}

/// Whether doctor's `--json` report has a `sessions.daemon` info line for the profile at
/// `profile` (its fix names the profile's path).
fn doctor_names_daemon(doctor: &Value, profile: &str) -> bool {
    doctor["checks"].as_array().is_some_and(|checks| {
        checks.iter().any(|c| {
            c["id"] == "sessions.daemon"
                && c["status"] == "info"
                && (c["fix"].as_str().is_some_and(|f| f.contains(profile))
                    || c["message"].as_str().is_some_and(|m| m.contains(profile)))
        })
    })
}

/// Doctor's `sessions.daemon` entries, whole (id, status, message, fix), as evidence.
fn daemon_entries(doctor: &Value) -> Value {
    let entries: Vec<Value> = doctor["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["id"] == "sessions.daemon")
        .map(|c| json!({"id": c["id"], "status": c["status"], "message": c["message"], "fix": c["fix"]}))
        .collect();
    json!(entries)
}

/// What the `--bg` sub-check makes of the daemon's stop. A stop that failed is a failed
/// expectation carrying its error, unless the run was cancelled: a cancelled stop is the
/// run's interruption, which must unwind (an Error outcome), not a verdict on the daemon.
fn stop_outcome(
    p: &mut Probe,
    stop: Result<Option<Value>, HarnessError>,
    cancelled: bool,
) -> Result<(), HarnessError> {
    match stop {
        Ok(ran) => p.note("claude daemon stop --any", json!(ran)),
        Err(e) if cancelled => return Err(e),
        Err(e) => {
            p.expect(
                "--bg: claude daemon stop --any stopped it",
                false,
                json!(e.0),
            );
        }
    }
    Ok(())
}

/// The `error.type` of a `--json` command's error object.
fn error_kind(v: &Value) -> Option<&str> {
    v["error"]["type"].as_str()
}

/// One spelling of CC's storage-write lock, watched while CC runs: how often its directory was
/// there, and how often tagteam's try-lock of it was refused.
struct Spelling {
    path: std::path::PathBuf,
    spec: MkdirLockSpec,
    seen: u32,
    refused: u32,
}

impl Spelling {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            spec: MkdirLockSpec::new(path.to_path_buf(), STORAGE_WRITE_STALE, Duration::ZERO),
            seen: 0,
            refused: 0,
        }
    }

    /// When the directory is there, tagteam tries it, once: a refusal is CC's lock excluding
    /// tagteam's.
    fn probe(&mut self) -> Result<(), HarnessError> {
        if self.path.is_dir() {
            self.seen += 1;
            match MkdirLock::try_acquire(&self.spec) {
                Ok(None) => self.refused += 1,
                Ok(Some(ours)) => drop(ours),
                Err(e) => return Err(harness(format!("trying the storage-write lock: {e}"))),
            }
        }
        Ok(())
    }

    fn tally(&self) -> Value {
        json!({"seen": self.seen, "refused": self.refused})
    }
}

/// Whether CC was seen holding either spelling and tagteam's try-lock of it was refused, and
/// which spelling that was (both when both).
fn lock_verdict(old: &Spelling, new: &Spelling) -> (bool, Vec<&'static str>) {
    let mut which = Vec::new();
    if old.refused > 0 {
        which.push(".storage-write");
    }
    if new.refused > 0 {
        which.push(".storage-write.lock");
    }
    (!which.is_empty(), which)
}

/// The storage-write lock's timestamp in the item (macOS) or the file's (Linux), epoch seconds.
fn written_at(ctx: &Ctx, spelling: &str) -> Option<i64> {
    match ctx.read_credential(spelling).ok()?? {
        (_, crate::compat::ctx::Place::Item(item)) => item.modified(),
        (_, crate::compat::ctx::Place::File(f)) => fs::metadata(f)
            .ok()?
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs() as i64),
    }
}

/// §9.1: the storage-write lock is the pair of directories `<secure-storage dir>/.storage-write`
/// and `.storage-write.lock` (the one CC 2.1.292 takes). While
/// tagteam holds it, CC's refresh does not write; once released, it does. While CC holds it
/// for its own write, tagteam's attempt finds it taken.
pub fn storage_write_lock(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let (_, spelling) = ctx.profile_ready()?;
    let paths = ctx.paths(&spelling);
    let generation_now = |ctx: &Ctx| {
        ctx.read_credential(&spelling)
            .ok()
            .flatten()
            .and_then(|(b, _)| generation(&b))
    };

    // CC waits for tagteam's.
    p.note("expired", ctx.expire(&spelling)?);
    let start = generation_now(ctx);
    let lock = acquire_storage_write(&paths, Duration::from_secs(9), &Cancel::new())
        .map_err(|e| harness(format!("the storage-write lock: {e}")))?;
    let running = ctx.run_as(ALIAS_OAUTH, &REQUEST).spawn(&ctx.roots)?;
    let refreshing = wait_until(Duration::from_secs(60), || paths.refresh_lock.is_dir());
    pause(HOLD);
    let held = generation_now(ctx);
    let released = now_ms() / 1000;
    drop(lock);
    let ran = running.wait()?;
    let after = generation_now(ctx);
    p.expect(
        "CC refreshed while tagteam held the lock",
        refreshing,
        json!(null),
    );
    p.expect_eq(
        "CC wrote nothing while tagteam held it",
        json!(start),
        json!(held),
    );
    p.expect(
        "CC wrote once tagteam released it",
        after.is_some() && after != start && written_at(ctx, &spelling).is_some_and(|t| t >= released),
        json!({"before": start, "after": after, "writtenAt": written_at(ctx, &spelling), "releasedAt": released}),
    );
    p.expect("claude -p succeeded", ran.success(), ran.summary());

    // tagteam waits for CC's: whenever CC's lock is there, tagteam's attempt fails.
    p.note("expired again", ctx.expire(&spelling)?);
    // Either spelling is excluded by tagteam's pair (§9.1): whichever one CC is seen holding is
    // try-locked, and must refuse tagteam. CC 2.1.292 is expected to hold `.storage-write.lock`.
    let mut old = Spelling::new(&paths.storage_write_lock);
    let mut new = Spelling::new(&paths.storage_write_lock_v2);
    let mut running = ctx.run_as(ALIAS_OAUTH, &REQUEST).spawn(&ctx.roots)?;
    let deadline = Instant::now() + Duration::from_secs(180);
    while !running.finished() && Instant::now() < deadline && cancel().requested().is_none() {
        old.probe()?;
        new.probe()?;
        thread::sleep(Duration::from_millis(1));
    }
    let ran = running.wait()?;
    for (name, spelling) in [(".storage-write", &old), (".storage-write.lock", &new)] {
        if spelling.seen > 0 && spelling.refused == 0 {
            p.note(
                &format!("CC's {name} was seen but never refused tagteam"),
                spelling.tally(),
            );
        }
    }
    let (held, which) = lock_verdict(&old, &new);
    p.expect(
        "CC held a storage-write lock under a spelling tagteam takes, and it refused tagteam",
        held,
        json!({"held": which, ".storage-write": old.tally(), ".storage-write.lock": new.tally()}),
    );
    p.expect("claude -p succeeded", ran.success(), ran.summary());
    Ok(p.finish("CC and tagteam each waited for the other's storage-write lock"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;

    use super::*;
    use crate::compat::ctx::Account;
    use crate::compat::layout::make_scratch;
    use crate::compat::report::{Redactor, Status};

    /// A `Ctx` whose profile exists (marker and global config) and whose `claude` is `script`.
    fn ctx_with_profile(script: &str) -> (Ctx, PathBuf, PathBuf) {
        let scratch = make_scratch().unwrap();
        let state = std::env::temp_dir().join(format!(
            "xtask-profile-{}",
            crate::compat::keychain::random_hex().unwrap()
        ));
        fs::create_dir_all(&state).unwrap();
        let state = fs::canonicalize(state).unwrap();
        let fake = scratch.join("claude");
        fs::write(
            &fake,
            format!("#!/bin/sh\nhome=\"$CLAUDE_CONFIG_DIR\"\n{script}\n"),
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let mut ctx = Ctx::offline(
            &scratch,
            &state,
            PathBuf::from("/nonexistent/tagteam"),
            Redactor::default(),
        );
        ctx.claude = fake;
        ctx.oauth = Account {
            id: "01a".into(),
            position: 1,
            email: "t@x.co".into(),
        };
        let profile = ctx.layout.profile("01a");
        fs::create_dir_all(&profile).unwrap();
        let profile = fs::canonicalize(profile).unwrap();
        fs::write(
            profile.join(".tagteam-profile.json"),
            json!({"format": "tagteam-profile", "version": 1, "provider": "claude-code",
                   "accountId": "01a", "configDir": profile, "outer": {}})
            .to_string(),
        )
        .unwrap();
        fs::write(profile.join(".claude.json"), "{}").unwrap();
        (ctx, scratch, state)
    }

    /// `--version` writes nothing; `auth status` creates CC's start-up files in a home with no
    /// global config and nothing in one that has it.
    const WELL_BEHAVED: &str = r#"case "$1" in
--version) echo "2.1.292 (Claude Code)" ;;
auth)
    if [ ! -e "$home/.claude.json" ]; then
        echo '{}' > "$home/.claude.json"; mkdir "$home/.claude.json.lock" "$home/backups"
    fi
    echo '{"loggedIn":false,"authMethod":"none"}'; exit 1 ;;
esac"#;

    #[test]
    fn doctor_and_switch_evidence_for_a_running_daemon_is_read_from_their_json() {
        let doctor = json!({"checks": [
            {"id": "keychain.lock", "status": "ok", "message": "m", "fix": null},
            {"id": "sessions.daemon", "status": "info", "message": "a daemon runs",
             "fix": "stop it with `claude daemon stop --any` run with CLAUDE_CONFIG_DIR set to '/p/one'"},
        ]});
        assert!(doctor_names_daemon(&doctor, "/p/one"));
        assert!(!doctor_names_daemon(&doctor, "/p/two"), "another profile");
        let warn = json!({"checks": [{"id": "sessions.daemon", "status": "warn",
                                      "message": "/p/one", "fix": null}]});
        assert!(!doctor_names_daemon(&warn, "/p/one"), "not an info line");
        assert!(!doctor_names_daemon(&json!({}), "/p/one"));

        let refused =
            json!({"schemaVersion": 1, "error": {"type": "session-owned", "message": "m"}});
        assert_eq!(error_kind(&refused), Some("session-owned"));
        assert_eq!(error_kind(&json!({"switched": true})), None);
    }

    #[test]
    fn cc_holding_either_spelling_and_refusing_tagteam_passes_and_names_it() {
        let dir = std::env::temp_dir().join(format!(
            "xtask-spelling-{}",
            crate::compat::keychain::random_hex().unwrap()
        ));
        fs::create_dir_all(&dir).unwrap();
        for name in [".storage-write", ".storage-write.lock"] {
            let (mut old, mut new) = (
                Spelling::new(&dir.join(".storage-write")),
                Spelling::new(&dir.join(".storage-write.lock")),
            );
            // Nothing there: nothing seen, no verdict.
            old.probe().unwrap();
            new.probe().unwrap();
            assert_eq!(lock_verdict(&old, &new), (false, vec![]));
            // CC's lock under `name`: it is seen, and tagteam is refused.
            fs::create_dir(dir.join(name)).unwrap();
            old.probe().unwrap();
            new.probe().unwrap();
            fs::remove_dir(dir.join(name)).unwrap();
            let (held, which) = lock_verdict(&old, &new);
            assert!(held, "{name}");
            assert_eq!(which, [name]);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_cancelled_stop_unwinds_and_any_other_failed_stop_is_a_failed_expectation() {
        let failed = || Err(harness("`claude daemon stop` failed: exit 1"));
        let mut p = Probe::new();
        assert!(
            stop_outcome(&mut p, failed(), true).is_err(),
            "cancelled: an error"
        );

        let mut p = Probe::new();
        stop_outcome(&mut p, failed(), false).unwrap();
        let outcome = p.finish("done");
        assert_eq!(outcome.status, Status::Fail);
        assert_eq!(outcome.evidence[0].ok, Some(false));
        assert!(outcome.evidence[0].value.to_string().contains("exit 1"));

        let mut p = Probe::new();
        stop_outcome(&mut p, Ok(Some(json!({"exit": 0}))), true).unwrap();
        assert_eq!(p.finish("done").status, Status::Pass);
    }

    #[test]
    fn doctor_s_daemon_entries_are_kept_whole_as_evidence() {
        let doctor = json!({"checks": [
            {"id": "keychain.lock", "status": "ok", "message": "m", "fix": null},
            {"id": "sessions.daemon", "status": "info", "message": "a daemon runs", "fix": "stop it"},
        ]});
        assert_eq!(
            daemon_entries(&doctor),
            json!([{"id": "sessions.daemon", "status": "info", "message": "a daemon runs",
                    "fix": "stop it"}])
        );
        assert_eq!(daemon_entries(&Value::Null), json!([]));
    }

    fn label<'o>(o: &'o Outcome, text: &str) -> &'o crate::compat::report::Evidence {
        o.evidence
            .iter()
            .find(|e| e.label.contains(text))
            .unwrap_or_else(|| panic!("no evidence line {text:?} in {:?}", o.evidence))
    }

    #[test]
    fn what_auth_status_creates_in_an_empty_home_is_evidence_and_a_home_with_a_config_must_stay_untouched()
     {
        let _serial = crate::compat::sys::serial();
        let (mut ctx, scratch, state) = ctx_with_profile(WELL_BEHAVED);
        let outcome = auth_status_read_only(&mut ctx).unwrap();
        assert_eq!(outcome.status, Status::Pass, "{:?}", outcome.evidence);
        let created = label(&outcome, "what claude auth status created")
            .value
            .to_string();
        for name in [".claude.json", ".claude.json.lock", "backups"] {
            assert!(created.contains(&format!("created {name}")), "{created}");
        }
        assert_eq!(
            label(&outcome, "what claude auth status created").ok,
            None,
            "expected, so no verdict"
        );
        let version = label(&outcome, "what claude --version created");
        assert_eq!(version.value, json!([]));
        assert_eq!(version.ok, None, "information only");
        assert!(ctx.layout.homes().join("read-only-version").is_dir());
        fs::remove_dir_all(&scratch).unwrap();
        fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn a_write_into_the_profile_or_into_a_home_that_has_its_config_fails_the_check() {
        let _serial = crate::compat::sys::serial();
        // Touches the profile.
        let (mut ctx, scratch, state) = ctx_with_profile(
            r#"case "$1" in auth) date +%s%N > "$home/touched"; echo '{}'; exit 1 ;; esac"#,
        );
        let outcome = auth_status_read_only(&mut ctx).unwrap();
        assert_eq!(outcome.status, Status::Fail);
        assert_eq!(
            label(&outcome, "the profile: nothing created").ok,
            Some(false)
        );
        fs::remove_dir_all(&scratch).unwrap();
        fs::remove_dir_all(&state).unwrap();

        // Touches only a home that already has its config (a second run).
        let (mut ctx, scratch, state) = ctx_with_profile(
            r#"case "$1" in
auth) if [ -e "$home/.claude.json" ] && [ "$home" != "$PROFILE" ]; then date +%s%N >> "$home/.claude.json"; else echo '{}' > "$home/.claude.json"; fi; echo '{}'; exit 1 ;;
esac"#,
        );
        let profile = ctx.profile().unwrap().unwrap().1;
        ctx.base.push(("PROFILE".into(), profile.into()));
        let outcome = auth_status_read_only(&mut ctx).unwrap();
        assert_eq!(outcome.status, Status::Fail);
        assert_eq!(
            label(&outcome, "the same home again, now").ok,
            Some(false),
            "{:?}",
            outcome.evidence
        );
        fs::remove_dir_all(&scratch).unwrap();
        fs::remove_dir_all(&state).unwrap();
    }
}
