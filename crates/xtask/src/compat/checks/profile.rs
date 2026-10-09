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
use tagteam_provider::{Cancel, MkdirLock, MkdirLockSpec, Read};

use super::{changes, new_record, read_json, records, snapshot};
use crate::compat::ctx::{ALIAS_OAUTH, ALIAS_SETUP_TOKEN, Ctx, MODEL, PROMPT, generation, now_ms};
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
        v["apiKeySource"].clone(),
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

/// §13.6 relies on it: `claude auth status` writes nothing in the home it inspects, a profile's
/// or an empty one, its Keychain items included.
pub fn auth_status_read_only(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    let mut p = Probe::new();
    let (_, profile) = ctx.profile_ready()?;
    let empty = ctx.new_home("read-only")?;
    for (label, home) in [("the profile", profile), ("an empty home", empty)] {
        let before = snapshot(Path::new(&home))?;
        let items = item_states(ctx, &home)?;
        let r = ctx.claude(&home, &STATUS).run(&ctx.roots)?;
        p.note(&format!("{label}: claude auth status"), r.summary());
        let found = changes(&before, &snapshot(Path::new(&home))?);
        p.expect(
            &format!("{label}: nothing created, changed or removed"),
            found.is_empty(),
            json!(found),
        );
        p.expect_eq(
            &format!("{label}: its Keychain items as they were"),
            items,
            item_states(ctx, &home)?,
        );
    }
    Ok(p.finish("claude auth status wrote nothing"))
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
/// that registers a `daemon` record in the profile.
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
    // the harness ends, so cleanup must stop it if this check cannot.
    ctx.must_stop(&spelling);
    let ran = ctx.run_profile(&["--bg", "--model", MODEL, PROMPT])?;
    p.note("claude --bg", ran.summary());
    let mut kinds = Vec::new();
    let daemon = wait_until(Duration::from_secs(60), || {
        kinds = records(&sessions)
            .into_iter()
            .filter_map(|(_, r)| r.kind)
            .collect();
        kinds.iter().any(|k| k == "daemon")
    });
    p.expect("--bg: a daemon record in the profile", daemon, json!(kinds));
    let stop = ctx
        .claude(&spelling, &["daemon", "stop"])
        .timeout(Duration::from_secs(60))
        .run(&ctx.roots)?;
    p.note("claude daemon stop", stop.summary());
    let quiet = wait_until(Duration::from_secs(60), || {
        records(&sessions)
            .iter()
            .all(|(_, r)| !matches!(r.kind.as_deref(), Some("daemon" | "bg" | "daemon-worker")))
    });
    p.expect("--bg: its records go when it stops", quiet, json!(null));
    Ok(p.finish("records come and go as Appendix A.7 says"))
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

/// §9.1: the storage-write lock is the directory `<secure-storage dir>/.storage-write`. While
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
    let spec = MkdirLockSpec::new(
        paths.storage_write_lock.clone(),
        STORAGE_WRITE_STALE,
        Duration::ZERO,
    );
    let mut running = ctx.run_as(ALIAS_OAUTH, &REQUEST).spawn(&ctx.roots)?;
    let (mut seen, mut refused) = (0u32, 0u32);
    let deadline = Instant::now() + Duration::from_secs(180);
    while !running.finished() && Instant::now() < deadline && cancel().requested().is_none() {
        if paths.storage_write_lock.is_dir() {
            seen += 1;
            match MkdirLock::try_acquire(&spec) {
                Ok(None) => refused += 1,
                Ok(Some(ours)) => drop(ours),
                Err(e) => return Err(harness(format!("trying the storage-write lock: {e}"))),
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
    let ran = running.wait()?;
    p.expect(
        "CC's lock is the directory tagteam names, and it refused tagteam",
        refused > 0,
        json!({"seen": seen, "refused": refused}),
    );
    p.expect("claude -p succeeded", ran.success(), ran.summary());
    Ok(p.finish("CC and tagteam each waited for the other's storage-write lock"))
}
