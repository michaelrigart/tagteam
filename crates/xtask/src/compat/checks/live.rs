//! The live phase: tagteam has made the test account the scratch default home's live login, so
//! the default home is where its lineage advances, and plain `claude` runs there. The checks
//! that must see which credential CC sends point it at the local stand-in (`capture`) and
//! spend nothing; a dummy API key and the setup token are the other credentials they switch to.

use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tagteam_cc::ItemKind;
use tagteam_cc::locks::{CONFIG_ACQUIRE_TIMEOUT, acquire_config};
use tagteam_provider::atomic::write_atomic_private;
use tagteam_provider::splice::{get_top_level, remove_top_level, replace_top_level};
use tagteam_provider::{Cancel, Read};

use super::profile::read_name;
use super::{keys, new_record, records};
use crate::compat::capture::{Capture, Sent};
use crate::compat::ctx::{
    ALIAS_API_KEY, ALIAS_OAUTH, ALIAS_SETUP_TOKEN, Ctx, MIN_LIFE_MS, MODEL, PROMPT, Prior, Restore,
    dummy_key, generation, life_to_spare, now_ms,
};
use crate::compat::report::{Outcome, Probe, fingerprint};
use crate::compat::store;
use crate::compat::sys::{HarnessError, harness, pause, wait_until};

const STATUS: [&str; 3] = ["auth", "status", "--json"];
const REQUEST: [&str; 6] = ["-p", PROMPT, "--model", MODEL, "--max-turns", "1"];
const HOLD: Duration = Duration::from_secs(3);
const MCP_SERVER: &str = "tagteam-compat-probe";

fn sent_json(s: Option<Sent>) -> Value {
    s.map_or(Value::Null, |s| s.to_json())
}

/// The access token `credential` carries, fingerprinted as the stand-in records it.
fn bearer(credential: &[u8]) -> Option<Sent> {
    let v: Value = serde_json::from_slice(credential).ok()?;
    v["claudeAiOauth"]["accessToken"]
        .as_str()
        .map(|t| Sent::Bearer(fingerprint(t)))
}

/// §9.5: CC accepts the global config tagteam created on a fresh machine (the activation that
/// opened this phase made it), and CC's next write of it re-renders, byte for byte, as tagteam's
/// splice renders each top-level value: that pins §9.5's rendering against `JSON.stringify`.
pub fn fresh_global_config(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    Ok(Probe::run(|p| {
        let live = ctx.live();
        let path = ctx.paths(&live).global_config;
        let created = ctx
            .activation_config
            .clone()
            .ok_or_else(|| harness("the activation's global config was not recorded"))?;
        let made: Value = serde_json::from_slice(&created)
            .map_err(|_| harness("tagteam's global config is not JSON"))?;
        p.expect_eq(
            "tagteam created it holding only oauthAccount (§9.5)",
            json!(["oauthAccount"]),
            json!(keys(&made)),
        );
        let ran = ctx.claude(&live, &REQUEST).run(&ctx.roots)?;
        p.expect("CC ran on it", ran.success(), ran.summary());
        let after = fs::read(&path)?;
        p.expect(
            "CC has written it since",
            after != created,
            json!({"bytesBefore": created.len(), "bytesAfter": after.len()}),
        );
        let doc: Value = serde_json::from_slice(&after)
            .map_err(|_| harness("CC left the global config unparseable"))?;
        p.expect(
            "the login is still the account's",
            doc["oauthAccount"]["emailAddress"] == made["oauthAccount"]["emailAddress"],
            json!(null),
        );
        let mut differ = Vec::new();
        for key in keys(&doc) {
            let spliced = replace_top_level(&after, &key, &doc[&key])
                .map_err(|e| harness(format!("splicing {key}: {e}")))?;
            if spliced != after {
                differ.push(key);
            }
        }
        p.expect(
            "every top-level value CC wrote renders as tagteam renders it",
            differ.is_empty(),
            json!(differ),
        );
        if doc["oauthAccount"] == made["oauthAccount"] {
            let same = replace_top_level(&after, "oauthAccount", &made["oauthAccount"])
                .map_err(|e| harness(format!("splicing oauthAccount: {e}")))?
                == after;
            p.expect(
                "the span tagteam spliced is byte-identical",
                same,
                json!(null),
            );
        } else {
            p.note(
                "CC changed oauthAccount itself; its fields",
                json!(keys(&doc["oauthAccount"])),
            );
        }
        let mut corrupt = Vec::new();
        for dir in [ctx.layout.live(), ctx.layout.live().join("backups")] {
            for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.contains("corrupt") {
                    corrupt.push(name);
                }
            }
        }
        p.expect(
            "CC set no copy aside as corrupt",
            corrupt.is_empty(),
            json!(corrupt),
        );
        Ok("CC took tagteam's file, and its write re-renders as tagteam's")
    }))
}

/// How long tagteam holds the config lock while CC's start-up is watched: well past the 1.5 s
/// CC retries it for, and past a slow start, yet inside CC's 30 s start-up phase, so a CC that
/// is slow to start still writes under the held lock and is not read as having waited it out.
const START_UP_WINDOW: Duration = Duration::from_secs(15);

/// §9.1 and Appendix A.7 (*2.1.292*): for its first 30 s, until its interactive UI is up, CC
/// retries `<global config>.lock` for only 1.5 s and then writes the global config without it,
/// unless every key it changes is one of its own counters or caches. So `claude mcp add --scope
/// user` (a new process, in its start-up, changing `mcpServers`) must write while tagteam still
/// holds the lock: that is the expectation. A write only after tagteam released the lock means
/// CC waited it out, which contradicts A.7, and fails the check; no write at all is a harness
/// error. The seconds, the window and whether the write landed inside it are notes. The lock
/// excludes only a CC process past its start-up (for its full retry window, about 10 s); that
/// half is not tested (see the note it records).
pub fn config_lock(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    config_lock_within(ctx, START_UP_WINDOW)
}

fn config_lock_within(ctx: &mut Ctx, window: Duration) -> Result<Outcome, HarnessError> {
    Ok(Probe::run(|p| {
        let live = ctx.live();
        let paths = ctx.paths(&live);
        let before = fs::read(&paths.global_config)?;
        let lock = acquire_config(&paths, CONFIG_ACQUIRE_TIMEOUT, &Cancel::new())
            .map_err(|e| harness(format!("CC's config lock: {e}")))?;
        let started = Instant::now();
        let mut running = ctx
            .claude(
                &live,
                &[
                    "mcp",
                    "add",
                    "--scope",
                    "user",
                    MCP_SERVER,
                    "--",
                    "/usr/bin/true",
                ],
            )
            .spawn(&ctx.roots)?;
        let wrote = wait_until(window, || {
            fs::read(&paths.global_config).is_ok_and(|now| now != before)
        });
        let seconds = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
        let finished_early = running.finished();
        drop(lock);
        let ran = running.wait()?;
        let after = fs::read(&paths.global_config)?;
        p.expect(
            "start-up: CC's write of the global config landed while tagteam still held .claude.json.lock (2.1.292, A.7)",
            wrote,
            json!(if wrote {
                "written under the held lock"
            } else if after != before {
                "written only after tagteam released it: CC waited the lock out"
            } else {
                "not written"
            }),
        );
        p.note(
            "start-up: when CC's write landed",
            json!({"seconds": seconds, "window": window.as_secs(), "insideWindow": wrote}),
        );
        p.note(
            "claude mcp add had finished while it was held",
            json!(finished_early),
        );
        if after == before {
            return Err(harness(format!(
                "claude mcp add wrote nothing to the global config, with the lock held or after: {}",
                ran.summary()
            )));
        }
        let added = serde_json::from_slice::<Value>(&after)
            .ok()
            .is_some_and(|v| v["mcpServers"].get(MCP_SERVER).is_some());
        p.expect("the server was added", added, ran.summary());
        p.note(
            "exclusion by the lock past CC's start-up",
            json!(
                "not tested: it holds only for a CC process more than 30 s old or with its interactive UI up, and compat drives no entry point there that makes a chosen change of the global config (mcp add and remove, and -p, end within their start-up or write only as they exit; an interactive session's own writes are CC's)"
            ),
        );
        let removed = ctx
            .claude(&live, &["mcp", "remove", "--scope", "user", MCP_SERVER])
            .run(&ctx.roots)?;
        p.note("claude mcp remove", removed.summary());
        Ok(
            "CC's start-up wrote the global config under tagteam's held lock, as 2.1.292's Appendix A.7 records",
        )
    }))
}

/// §7.5 and §9.1: when CC and tagteam both find the live access token expired, CC's credential
/// locks serialize them, whichever starts first, and no generation is refreshed twice.
/// - CC first: CC refreshes holding the refresh lock. `tagteam list` then takes one of two
///   paths, both correct, and which one is a race real `claude` cannot be held to: it reads the
///   token still expired, waits on CC's credential locks (§9.1) and adopts CC's rotation into
///   the vault without a request (§7.5 steps 3 and 4), or it reads CC's fresh token and leaves
///   the active token to CC (§7.5). So only what holds on both is expected: both succeed, the
///   live store holds CC's new generation, tagteam sent no refresh of its own (resending the
///   generation CC consumed would fail, R9, and show as a refresh failure), and the vault holds
///   CC's generation or the one it held before. Which path ran is recorded.
/// - tagteam first: tagteam stops at its `active-before-request` point with the locks held
///   (`TAGTEAM_TEST_PAUSE_AT`); CC waits, then sends with the generation tagteam wrote, so both
///   end on one generation.
///
/// `tagteam list` runs the active refresh only inside a collection, and §8.3's floor and the
/// plan the last collection or switch recorded would hold it back, so each order first makes
/// the account due (`store::make_due`).
pub fn refresh_lock_interop(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    Ok(Probe::run(|p| {
        let ctx: &Ctx = ctx;
        let token = LiveToken::new(ctx);
        let done = interop(ctx, p, &token);
        // However it ended, the live token is left fresh for the checks after it: an error
        // between `expire` and CC's refresh would leave it expired, and CC would refresh it
        // under whatever runs next (hot reload judged the wrong token because of that). A
        // panic in `interop` is covered by `LiveToken`'s drop.
        settle_token(p, done, token.restore())
    }))
}

/// How the interop check ends, given its own result and the restore of the live token.
fn settle_token(
    p: &mut Probe,
    done: Result<&'static str, HarnessError>,
    restored: Result<Option<Restore>, HarnessError>,
) -> Result<&'static str, HarnessError> {
    let note = |p: &mut Probe, restored: &Option<Restore>| match restored {
        None => p.note(
            "the live token: nothing to restore, since no expiry with life to spare was replaced",
            json!(null),
        ),
        Some(Restore::Restored) => p.note(
            "the live token was still expired; its expiry before the check is put back",
            json!(null),
        ),
        Some(Restore::Fresh) => {}
        Some(Restore::Changed) => p.note(
            "the live token is another generation than the one the check expired; left as it is",
            json!(null),
        ),
    };
    match (done, restored) {
        (Ok(summary), Ok(restored)) => {
            note(p, &restored);
            Ok(summary)
        }
        // Leaving the token fresh failed after a check that otherwise ran: the next checks
        // cannot be trusted, so this is the harness failing, not a finding about CC.
        (Ok(_), Err(e)) => Err(e),
        (Err(e), Ok(restored)) => {
            note(p, &restored);
            Err(e)
        }
        (Err(e), Err(not_fresh)) => {
            p.note("the live token could not be left fresh", json!(not_fresh.0));
            Err(e)
        }
    }
}

/// The live home's token as the interop check expires it, and the guard that leaves it fresh:
/// `restore` puts back the expiry `expire` replaced, and dropping the guard does the same on
/// an unwind, ignoring the outcome.
struct LiveToken<'a> {
    ctx: &'a Ctx,
    live: String,
    prior: RefCell<Option<Prior>>,
}

impl<'a> LiveToken<'a> {
    fn new(ctx: &'a Ctx) -> Self {
        Self {
            ctx,
            live: ctx.live(),
            prior: RefCell::new(None),
        }
    }

    /// `Ctx::expire` of the live home, remembering what it replaced when that had life to spare.
    fn expire(&self) -> Result<Value, HarnessError> {
        let (note, prior) = self.ctx.expire_noting(&self.live)?;
        if let Some(prior) = prior.filter(|p| p.expires_at - now_ms() >= MIN_LIFE_MS) {
            *self.prior.borrow_mut() = Some(prior);
        }
        Ok(note)
    }

    /// `None` when nothing is remembered to restore.
    fn restore(&self) -> Result<Option<Restore>, HarnessError> {
        let prior = self.prior.borrow_mut().take();
        prior
            .map(|prior| self.ctx.unexpire(&self.live, &prior))
            .transpose()
    }
}

impl Drop for LiveToken<'_> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn interop(ctx: &Ctx, p: &mut Probe, token: &LiveToken) -> Result<&'static str, HarnessError> {
    let live = ctx.live();
    let paths = ctx.paths(&live);
    let db = ctx.layout.data_dir().join("tagteam.db");
    let vault_generation = |ctx: &Ctx| -> Result<Option<String>, HarnessError> {
        Ok(generation(&ctx.vault_credential(&ctx.oauth.id)?))
    };
    let live_generation = |ctx: &Ctx| -> Result<Option<String>, HarnessError> {
        Ok(ctx
            .read_credential(&ctx.live())?
            .and_then(|(b, _)| generation(&b)))
    };

    // CC first.
    store::make_due(&db, &ctx.oauth.id, now_ms() / 1000)?;
    let expired = token.expire()?;
    let consumed = expired["generation"].clone();
    p.note("expired", expired);
    let vault_before = vault_generation(ctx)?;
    let cc = ctx.claude(&live, &REQUEST).spawn(&ctx.roots)?;
    let locked = wait_until(Duration::from_secs(60), || paths.refresh_lock.is_dir());
    let still_held = paths.refresh_lock.is_dir();
    let live_at_start = live_generation(ctx)?;
    let tt = ctx.tagteam(&["list", "--json"]).run(&ctx.roots)?;
    let cc = cc.wait()?;
    p.expect("CC first: CC took the refresh lock", locked, json!(null));
    p.expect("CC first: claude -p succeeded", cc.success(), cc.summary());
    p.expect(
        "CC first: tagteam list succeeded",
        tt.success(),
        tt.summary(),
    );
    let live_after = live_generation(ctx)?;
    let vault_after = vault_generation(ctx)?;
    p.expect(
        "CC first: the live store holds CC's new generation, not the one its refresh consumed (§7.5)",
        live_after.is_some() && json!(live_after) != consumed,
        json!({"consumed": consumed, "live": live_after}),
    );
    let row = tt
        .json()
        .and_then(|v| {
            v["accounts"]
                .as_array()?
                .iter()
                .find(|r| r["alias"] == ALIAS_OAUTH)
                .cloned()
        })
        .unwrap_or(Value::Null);
    let refresh_failed =
        row["usageError"] == "refresh-failed" || row["usageStatus"] == "relogin_required";
    p.expect(
        "CC first: tagteam sent no refresh of its own: its row shows no refresh failure, which resending the consumed generation would be (R9, §7.5 step 4)",
        !refresh_failed,
        ctx.redact
            .value(&json!({"usageStatus": row["usageStatus"], "usageError": row["usageError"]})),
    );
    p.expect(
        "CC first: the vault holds CC's generation or the one it held before, never an older one over a newer (§7.5 step 3)",
        vault_after.is_some() && (vault_after == live_after || vault_after == vault_before),
        json!({"before": vault_before, "after": vault_after, "live": live_after}),
    );
    let path = if vault_after == live_after && vault_before != live_after {
        "tagteam waited for CC's locks and adopted CC's generation into the vault, sending nothing (§7.5 steps 3 and 4)"
    } else if vault_after == vault_before {
        "tagteam found CC's fresh token and left it to CC: no active refresh ran (§7.5)"
    } else {
        "neither"
    };
    p.note(
        "CC first: the path tagteam took",
        json!({"path": path, "lockHeldAtStart": still_held, "liveAtStart": live_at_start}),
    );

    // tagteam first.
    p.note("expired again", token.expire()?);
    let pause_dir = ctx.layout.scratch.join("pause");
    let _ = fs::remove_dir_all(&pause_dir);
    fs::create_dir(&pause_dir)?;
    store::make_due(&db, &ctx.oauth.id, now_ms() / 1000)?;
    let mut tt = ctx
        .tagteam(&["list", "--json"])
        .var("TAGTEAM_TEST_PAUSE_AT", "active-before-request")
        .var("TAGTEAM_TEST_PAUSE_DIR", &pause_dir)
        .timeout(Duration::from_secs(120))
        .spawn(&ctx.roots)?;
    wait_until(Duration::from_secs(60), || {
        pause_dir.join("paused").exists() || tt.finished()
    });
    let paused = pause_dir.join("paused").exists();
    if !paused {
        let why = if tt.finished() {
            tt.wait()?.summary()
        } else {
            json!("it was still running after 60 s")
        };
        return Err(harness(format!(
            "tagteam first: tagteam list never reached active-before-request, so this order proves nothing: {why}"
        )));
    }
    let locked = paths.refresh_lock.is_dir();
    let mut cc = ctx.claude(&live, &REQUEST).spawn(&ctx.roots)?;
    pause(HOLD);
    let cc_early = cc.finished();
    fs::write(pause_dir.join("resume"), b"")?;
    let tt = tt.wait()?;
    let cc = cc.wait()?;
    p.expect(
        "tagteam first: tagteam stopped holding CC's refresh lock",
        paused && locked,
        json!({"paused": paused, "locked": locked}),
    );
    p.expect("tagteam first: CC waited for it", !cc_early, json!(null));
    p.expect(
        "tagteam first: tagteam list succeeded",
        tt.success(),
        tt.summary(),
    );
    p.expect(
        "tagteam first: claude -p succeeded",
        cc.success(),
        cc.summary(),
    );
    let (vault, held) = (vault_generation(ctx)?, live_generation(ctx)?);
    p.expect(
        "tagteam first: the live store and the vault hold one generation",
        vault.is_some() && vault == held,
        json!({"vault": vault, "live": held}),
    );
    Ok("CC's credential locks kept one refresh at a time, both ways")
}

/// An interactive `claude` in the default home that talks to `capture`, started and ready.
fn session_on(
    ctx: &Ctx,
    capture: &Capture,
) -> Result<Option<crate::compat::sys::Pty>, HarnessError> {
    let live = ctx.live();
    let sessions = Path::new(&live).join("sessions");
    let before = records(&sessions);
    let pty = ctx
        .claude(&live, &["--model", MODEL])
        .var("ANTHROPIC_BASE_URL", capture.base_url())
        .pty(&ctx.roots)?;
    if new_record(&sessions, &before, Duration::from_secs(60)).is_none() {
        pty.finish(Duration::from_secs(5))?;
        return Ok(None);
    }
    pause(Duration::from_secs(2));
    Ok(Some(pty))
}

/// One prompt in `pty`; the credential its request carried.
fn prompt(
    pty: &mut crate::compat::sys::Pty,
    capture: &Capture,
) -> Result<Option<Sent>, HarnessError> {
    let n = capture.messages();
    pty.line(PROMPT)?;
    Ok(capture.next_message(n, Duration::from_secs(60)))
}

/// Appendix A.3 and §9.4's hint: a running `claude` takes up a switch at once when the
/// credential file's mtime moves, and from the Keychain within 30 s. The switch goes from the
/// account to its setup token and back, so the lineage never leaves the default home.
pub fn hot_reload(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    Ok(Probe::run(|p| {
        let Some(st) = ctx.setup_token.clone() else {
            p.expect(
                "a setup-token account to switch to",
                false,
                json!("run `cargo xtask compat login` again to add it"),
            );
            return Ok("");
        };
        let live = ctx.live();
        ctx.seed_trust(&live)?;
        let setup = bearer(&ctx.vault_credential(&st.id)?)
            .ok_or_else(|| harness("the setup token's vault entry has no access token"))?;
        let capture = Capture::start()?;
        let file = ctx.paths(&live).credentials_file;
        let modes: &[(&str, bool)] = if ctx.macos {
            &[("from the Keychain", false), ("by the file's mtime", true)]
        } else {
            &[("by the file's mtime", true)]
        };
        for &(label, with_file) in modes {
            ctx.switch(ALIAS_OAUTH)?;
            let (bytes, _) = ctx
                .read_credential(&live)?
                .ok_or_else(|| harness("the default home holds no credential"))?;
            // CC refreshes an expired token before its first message, which moves the lineage
            // this check judges: a precondition of the harness, not a finding.
            p.note(
                &format!("{label}: the live token's life left, in seconds"),
                json!(life_to_spare(&bytes, now_ms())? / 1000),
            );
            if ctx.macos {
                // tagteam rewrites the file after a Keychain write only if it exists (A.3).
                if with_file {
                    write_atomic_private(&file, &bytes, 0o600)?;
                } else if file.exists() {
                    fs::remove_file(&file)?;
                }
            }
            let Some(mut pty) = session_on(ctx, &capture)? else {
                p.expect(&format!("{label}: the session started"), false, json!(null));
                continue;
            };
            let first = prompt(&mut pty, &capture)?;
            // The account's token is whatever its refresh-token lineage holds in the live store
            // once the message is out, so a refresh CC makes anyway does not mislead the check.
            let (after, _) = ctx
                .read_credential(&live)?
                .ok_or_else(|| harness("the default home holds no credential"))?;
            if generation(&after) != generation(&bytes) {
                p.note(
                    &format!("{label}: CC refreshed before its first message"),
                    json!({"before": generation(&bytes), "after": generation(&after)}),
                );
            }
            p.expect_eq(
                &format!("{label}: before the switch, the account's token"),
                sent_json(bearer(&after)),
                sent_json(first),
            );
            p.note(&format!("{label}: switch"), ctx.switch(ALIAS_SETUP_TOKEN)?);
            let switched = Instant::now();
            if with_file {
                let next = prompt(&mut pty, &capture)?;
                p.expect_eq(
                    &format!("{label}: the next message carries the setup token"),
                    setup.to_json(),
                    sent_json(next),
                );
            } else {
                pause(Duration::from_secs(2));
                let early = prompt(&mut pty, &capture)?;
                p.note(&format!("{label}: 2 s after the switch"), sent_json(early));
                pause(Duration::from_secs(31).saturating_sub(switched.elapsed()));
                let late = prompt(&mut pty, &capture)?;
                p.expect_eq(
                    &format!("{label}: 31 s after the switch, the setup token"),
                    setup.to_json(),
                    sent_json(late),
                );
            }
            let _ = pty.line("/exit");
            pty.finish(Duration::from_secs(30))?;
            ctx.switch(ALIAS_OAUTH)?;
        }
        if ctx.macos && file.exists() {
            fs::remove_file(&file)?;
        }
        Ok("a running claude took up each switch as tagteam's hint says")
    }))
}

/// `primaryApiKey` in the home's global config, set or removed, with a set key's last 20
/// characters approved as tagteam approves its own (§9.4 step 7). Under CC's config lock.
fn set_primary_api_key(ctx: &Ctx, key: Option<&str>) -> Result<(), HarnessError> {
    let paths = ctx.paths(&ctx.live());
    let _lock = acquire_config(&paths, CONFIG_ACQUIRE_TIMEOUT, &Cancel::new())
        .map_err(|e| harness(format!("CC's config lock: {e}")))?;
    let doc = fs::read(&paths.global_config)?;
    let err = |e| harness(format!("splicing the global config: {e}"));
    let doc = match key {
        None => remove_top_level(&doc, "primaryApiKey").map_err(err)?,
        Some(k) => {
            let mut responses = get_top_level(&doc, "customApiKeyResponses")
                .map_err(err)?
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({"approved": [], "rejected": []}));
            let tail = &k[k.len().saturating_sub(20)..];
            if !responses["approved"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == tail))
            {
                if !responses["approved"].is_array() {
                    responses["approved"] = json!([]);
                }
                responses["approved"]
                    .as_array_mut()
                    .expect("an array")
                    .push(json!(tail));
            }
            let doc = replace_top_level(&doc, "customApiKeyResponses", &responses).map_err(err)?;
            replace_top_level(&doc, "primaryApiKey", &json!(k)).map_err(err)?
        }
    };
    write_atomic_private(&paths.global_config, &doc, 0o600)?;
    Ok(())
}

/// §9.4 step 7: activating an API key drops the credential entry's account-scoped keys and keeps
/// its machine-shared ones, and CC then runs on the key: it sends it, to the stand-in.
pub fn api_key_entry(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    Ok(Probe::run(|p| {
        let live = ctx.live();
        let (bytes, place) = ctx
            .read_credential(&live)?
            .ok_or_else(|| harness("the default home holds no credential"))?;
        let mut v: Value = serde_json::from_slice(&bytes)
            .map_err(|_| harness("the live credential is not JSON"))?;
        v["mcpOAuth"] = json!({"tagteam-compat|0": {
            "serverName": "tagteam-compat",
            "serverUrl": "http://127.0.0.1:9/mcp",
            "accessToken": "tagteam-compat-mcp-dummy",
            "expiresAt": 0
        }});
        ctx.write_credential(
            &live,
            &place,
            &serde_json::to_vec(&v).expect("a Value serializes"),
        )?;
        let key = ctx.add_dummy_api_key()?;
        p.note("switch", ctx.switch(ALIAS_API_KEY)?);
        match ctx.read_credential(&live)? {
            Some((b, at)) => {
                let entry: Value = serde_json::from_slice(&b).unwrap_or(Value::Null);
                p.expect_eq(
                    "the entry keeps only its machine-shared keys",
                    json!(["mcpOAuth"]),
                    json!(keys(&entry)),
                );
                p.note("the entry", at.describe());
            }
            None => {
                p.expect(
                    "the entry is kept for its machine-shared key",
                    false,
                    json!(null),
                );
            }
        }
        if ctx.macos {
            let stored = ctx.item(&live, ItemKind::ManagedKey)?.read();
            p.expect(
                "the key is in the managed-key item",
                matches!(&stored, Read::Present(b) if b.as_slice() == key.as_bytes()),
                json!(read_name(&stored)),
            );
        } else {
            let stored = get_top_level(&fs::read(ctx.paths(&live).global_config)?, "primaryApiKey")
                .ok()
                .flatten();
            p.expect(
                "the key is primaryApiKey",
                stored == Some(json!(key)),
                json!(stored.is_some()),
            );
        }
        let v = ctx
            .claude(&live, &STATUS)
            .run(&ctx.roots)?
            .json()
            .unwrap_or(Value::Null);
        p.expect_eq(
            "auth status: authMethod",
            "api_key",
            v["authMethod"].clone(),
        );
        p.note(
            "auth status: apiKeySource",
            ctx.redact.value(&v["apiKeySource"]),
        );
        let capture = Capture::start()?;
        let ran = ctx
            .claude(&live, &REQUEST)
            .var("ANTHROPIC_BASE_URL", capture.base_url())
            .timeout(Duration::from_secs(120))
            .run(&ctx.roots)?;
        p.note("claude -p against the stand-in", ran.summary());
        p.expect_eq(
            "CC sent the key",
            Sent::ApiKey(fingerprint(&key)).to_json(),
            sent_json(capture.next_message(0, Duration::from_secs(5))),
        );
        ctx.drop_dummy_api_key()?;
        if let Some((b, at)) = ctx.read_credential(&live)? {
            let mut entry: Value = serde_json::from_slice(&b).unwrap_or(Value::Null);
            if let Some(o) = entry.as_object_mut() {
                o.remove("mcpOAuth");
            }
            ctx.write_credential(
                &live,
                &at,
                &serde_json::to_vec(&entry).expect("a Value serializes"),
            )?;
        }
        Ok("the entry kept its machine-shared key and CC ran on the key")
    }))
}

/// §9.4 and Appendix A.7 (*2.1.292*): with the managed-key item and `primaryApiKey` both set,
/// which CC sends; with the item gone, `primaryApiKey`; and CC reads the managed key once and
/// caches it for the life of the process, so when `primaryApiKey` changes a running session keeps
/// sending the key it started with and a new process sends the changed one, which is why the
/// hint for an API-key account is "restart Claude Code to apply".
pub fn managed_key_precedence(ctx: &mut Ctx) -> Result<Outcome, HarnessError> {
    Ok(Probe::run(|p| {
        let live = ctx.live();
        ctx.seed_trust(&live)?;
        let k1 = ctx.add_dummy_api_key()?;
        p.note("switch", ctx.switch(ALIAS_API_KEY)?);
        let k2 = dummy_key("k2")?;
        set_primary_api_key(ctx, Some(&k2))?;
        let capture = Capture::start()?;
        let request = |ctx: &Ctx| -> Result<Option<Sent>, HarnessError> {
            let n = capture.messages();
            ctx.claude(&ctx.live(), &REQUEST)
                .var("ANTHROPIC_BASE_URL", capture.base_url())
                .timeout(Duration::from_secs(120))
                .run(&ctx.roots)?;
            Ok(capture.next_message(n, Duration::from_secs(5)))
        };
        p.expect_eq(
            "both set: CC sends the managed-key item's key",
            Sent::ApiKey(fingerprint(&k1)).to_json(),
            sent_json(request(ctx)?),
        );
        ctx.item(&live, ItemKind::ManagedKey)?.delete()?;
        p.expect_eq(
            "the item gone: CC sends primaryApiKey",
            Sent::ApiKey(fingerprint(&k2)).to_json(),
            sent_json(request(ctx)?),
        );
        let k3 = dummy_key("k3")?;
        let mut changed = false;
        match session_on(ctx, &capture)? {
            None => {
                p.expect("a session: it started", false, json!(null));
            }
            Some(mut pty) => {
                p.expect_eq(
                    "a session: its first message carries primaryApiKey",
                    Sent::ApiKey(fingerprint(&k2)).to_json(),
                    sent_json(prompt(&mut pty, &capture)?),
                );
                set_primary_api_key(ctx, Some(&k3))?;
                p.expect_eq(
                    "a session: the next message still carries the key it started with (cached for the process, 2.1.292)",
                    Sent::ApiKey(fingerprint(&k2)).to_json(),
                    sent_json(prompt(&mut pty, &capture)?),
                );
                let _ = pty.line("/exit");
                pty.finish(Duration::from_secs(30))?;
                changed = true;
            }
        }
        // After the session has ended, so that no request of its can be read as the new
        // process's.
        if changed {
            p.expect_eq(
                "a new process: it carries the changed primaryApiKey",
                Sent::ApiKey(fingerprint(&k3)).to_json(),
                sent_json(request(ctx)?),
            );
        }
        set_primary_api_key(ctx, None)?;
        ctx.drop_dummy_api_key()?;
        Ok(
            "CC read the managed keys in the order tagteam assumes, and a changed key reached only a new process",
        )
    }))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;

    use super::*;
    use crate::compat::report::{Redactor, Status};

    /// A `Ctx` whose default home holds an empty global config and whose `claude` is `script`.
    fn ctx_with_claude(script: &str) -> (Ctx, PathBuf) {
        let scratch = crate::compat::layout::make_scratch().unwrap();
        let fake = scratch.join("claude");
        fs::write(
            &fake,
            format!("#!/bin/sh\nhome=\"$CLAUDE_CONFIG_DIR\"\n{script}\n"),
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let mut ctx = Ctx::offline(
            &scratch,
            Path::new("/nonexistent/state"),
            PathBuf::from("/nonexistent/tagteam"),
            Redactor::default(),
        );
        ctx.claude = fake;
        fs::write(ctx.layout.live().join(".claude.json"), "{}\n").unwrap();
        (ctx, scratch)
    }

    const ADDS_AT_ONCE: &str = r#"case "$1 $2" in
"mcp add") printf '{"mcpServers":{"tagteam-compat-probe":{}}}' > "$home/.claude.json" ;;
esac"#;

    const WAITS_FOR_THE_LOCK: &str = r#"case "$1 $2" in
"mcp add")
    while [ -d "$home/.claude.json.lock" ]; do sleep 0.1; done
    printf '{"mcpServers":{"tagteam-compat-probe":{}}}' > "$home/.claude.json" ;;
esac"#;

    fn label<'o>(o: &'o Outcome, text: &str) -> &'o crate::compat::report::Evidence {
        o.evidence
            .iter()
            .find(|e| e.label.contains(text))
            .unwrap_or_else(|| panic!("no evidence line {text:?} in {:?}", o.evidence))
    }

    #[test]
    fn a_start_up_that_writes_past_the_held_lock_is_the_expected_behaviour() {
        let _serial = crate::compat::sys::serial();
        let (mut ctx, scratch) = ctx_with_claude(ADDS_AT_ONCE);
        let out = config_lock_within(&mut ctx, Duration::from_secs(5)).unwrap();
        assert_eq!(out.status, Status::Pass, "{:?}", out.evidence);
        assert_eq!(
            label(&out, "start-up: CC's write of the global config landed").ok,
            Some(true)
        );
        assert_eq!(
            label(&out, "start-up: when CC's write landed").value["insideWindow"],
            json!(true)
        );
        assert_eq!(label(&out, "the server was added").ok, Some(true));
        let note = label(&out, "exclusion by the lock past CC's start-up");
        assert_eq!(note.ok, None, "a note, not an expectation");
        assert!(note.value.to_string().contains("not tested"));
        assert!(
            !ctx.paths(&ctx.live()).config_lock.exists(),
            "the lock is released"
        );
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_write_only_after_the_release_means_cc_waited_the_lock_out_and_fails() {
        let _serial = crate::compat::sys::serial();
        let (mut ctx, scratch) = ctx_with_claude(WAITS_FOR_THE_LOCK);
        let out = config_lock_within(&mut ctx, Duration::from_millis(800)).unwrap();
        assert_eq!(out.status, Status::Fail, "{:?}", out.evidence);
        let landed = label(&out, "start-up: CC's write of the global config landed");
        assert_eq!(landed.ok, Some(false));
        assert!(landed.value.to_string().contains("waited the lock out"));
        let when = label(&out, "start-up: when CC's write landed");
        assert_eq!(
            (when.ok, &when.value["insideWindow"]),
            (None, &json!(false))
        );
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn no_write_at_all_is_a_harness_error_that_keeps_the_evidence() {
        let _serial = crate::compat::sys::serial();
        let (mut ctx, scratch) = ctx_with_claude("exit 0");
        let out = config_lock_within(&mut ctx, Duration::from_millis(300)).unwrap();
        assert_eq!(out.status, Status::Error, "{:?}", out.evidence);
        assert!(out.summary.contains("wrote nothing"), "{}", out.summary);
        let landed = label(&out, "start-up: CC's write of the global config landed");
        assert_eq!(landed.ok, Some(false));
        assert!(!ctx.paths(&ctx.live()).config_lock.exists());
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn an_error_after_the_first_half_keeps_its_evidence_and_leaves_the_live_token_fresh() {
        let _serial = crate::compat::sys::serial();
        let (mut ctx, scratch) = ctx_with_claude("exit 0");
        let live = ctx.live();
        let file = Path::new(&live).join(".credentials.json");
        let was = now_ms() + 8 * 3_600_000;
        fs::write(
            &file,
            format!(
                r#"{{"claudeAiOauth":{{"accessToken":"at","refreshToken":"rt","expiresAt":{was}}}}}"#
            ),
        )
        .unwrap();
        // A store with no vault: the check expires the token, then meets a harness error.
        ctx.layout.state = scratch.join("state");
        let data = ctx.layout.data_dir();
        fs::create_dir_all(&data).unwrap();
        rusqlite::Connection::open(data.join("tagteam.db"))
            .unwrap()
            .execute_batch(include_str!(
                "../../../../tagteam-engine/src/store/schema.sql"
            ))
            .unwrap();
        let out = refresh_lock_interop(&mut ctx).unwrap();
        assert_eq!(out.status, Status::Error, "{:?}", out.evidence);
        assert!(out.summary.contains("vault file"), "{}", out.summary);
        assert_eq!(
            label(&out, "expired").ok,
            None,
            "the evidence gathered before the error is kept"
        );
        let bytes = fs::read(&file).unwrap();
        assert_eq!(
            crate::compat::ctx::expires_at(&bytes),
            Some(was),
            "the live token was left fresh"
        );
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_panic_inside_the_check_still_leaves_the_live_token_fresh() {
        let _serial = crate::compat::sys::serial();
        let (ctx, scratch) = ctx_with_claude("exit 0");
        let live = ctx.live();
        let file = Path::new(&live).join(".credentials.json");
        let was = now_ms() + 8 * 3_600_000;
        fs::write(
            &file,
            format!(
                r#"{{"claudeAiOauth":{{"accessToken":"at","refreshToken":"rt","expiresAt":{was}}}}}"#
            ),
        )
        .unwrap();
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let token = LiveToken::new(&ctx);
            token.expire().unwrap();
            assert!(crate::compat::ctx::expires_at(&fs::read(&file).unwrap()).unwrap() < now_ms());
            panic!("the check panicked");
        }));
        assert!(unwound.is_err());
        assert_eq!(
            crate::compat::ctx::expires_at(&fs::read(&file).unwrap()),
            Some(was)
        );
        // Nothing remembered, nothing restored.
        let token = LiveToken::new(&ctx);
        assert_eq!(token.restore(), Ok(None));
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_token_with_no_life_to_begin_with_is_noted_as_nothing_to_restore() {
        let _serial = crate::compat::sys::serial();
        let (mut ctx, scratch) = ctx_with_claude("exit 0");
        let live = ctx.live();
        let file = Path::new(&live).join(".credentials.json");
        // The token has little life left, so expiring it records nothing to restore; the
        // harness error is the run's own (no vault).
        fs::write(
            &file,
            r#"{"claudeAiOauth":{"accessToken":"at","refreshToken":"rt","expiresAt":1}}"#,
        )
        .unwrap();
        ctx.layout.state = scratch.join("state");
        let data = ctx.layout.data_dir();
        fs::create_dir_all(&data).unwrap();
        rusqlite::Connection::open(data.join("tagteam.db"))
            .unwrap()
            .execute_batch(include_str!(
                "../../../../tagteam-engine/src/store/schema.sql"
            ))
            .unwrap();
        let out = refresh_lock_interop(&mut ctx).unwrap();
        assert_eq!(out.status, Status::Error);
        assert_eq!(
            label(&out, "the live token: nothing to restore").ok,
            None,
            "said so, since no expiry with life to spare was replaced"
        );
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_failed_restore_is_an_error_after_a_check_that_ran_and_a_note_after_one_that_did_not() {
        let fail = || Err(harness("could not leave it fresh"));
        let mut p = Probe::new();
        let e = settle_token(&mut p, Ok("fine"), fail()).unwrap_err();
        assert_eq!(e.0, "could not leave it fresh");

        let mut p = Probe::new();
        let e = settle_token(&mut p, Err(harness("the run's own")), fail()).unwrap_err();
        assert_eq!(e.0, "the run's own");
        let out = Outcome::error(e.0, p.into_evidence());
        assert_eq!(
            label(&out, "the live token could not be left fresh").value,
            json!("could not leave it fresh")
        );

        let mut p = Probe::new();
        assert_eq!(
            settle_token(&mut p, Ok("fine"), Ok(Some(Restore::Restored))),
            Ok("fine")
        );
        assert_eq!(
            settle_token(&mut p, Ok("fine"), Ok(Some(Restore::Fresh))),
            Ok("fine")
        );
        let out = p.finish("done");
        assert_eq!(out.evidence.len(), 1, "only a restore is noted");
    }
}
